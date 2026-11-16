mod bridge;

use std::sync::Arc;

use bridge::{BridgeClient, BridgeEndpoint, EndpointSource, resolve_endpoint_source};
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
use tokio_util::sync::CancellationToken;

/// Upper bound for the client-supplied waitMs so a single call cannot stall a
/// client indefinitely; longer jobs fall back to the regular poll flow.
const MAX_WAIT_MS: u64 = 30_000;

/// Default bounded wait for terminal_start when the client does not specify
/// one: a plain call returns the command's outcome directly, so models need
/// no polling loop and no prior knowledge. Clients opt out with waitMs: 0.
const DEFAULT_WAIT_MS: u64 = 15_000;

/// Job statuses that can still transition; only these keep a waitMs loop going.
const TRANSIENT_JOB_STATUSES: &[&str] = &["starting", "running", "cancelling"];

#[derive(Clone)]
struct NyaTermMcp {
    bridge: BridgeClient,
    tools: Arc<Vec<Tool>>,
}

impl NyaTermMcp {
    /// Wait for a started job to reach a terminal state so clients can fetch a
    /// command's outcome in one call instead of busy-polling terminal_poll.
    /// On timeout or a polling error the last known result is returned and the
    /// client can keep polling; the job itself keeps running on the host.
    async fn await_job_result(
        &self,
        start: Value,
        wait_ms: u64,
        cancellation: CancellationToken,
    ) -> Value {
        let budget = std::time::Duration::from_millis(wait_ms.min(MAX_WAIT_MS));
        let mut waited = std::time::Duration::ZERO;
        let mut backoff = std::time::Duration::from_millis(150);
        let mut latest = start;
        loop {
            let transient = matches!(
                latest.get("status").and_then(Value::as_str),
                Some(status) if TRANSIENT_JOB_STATUSES.contains(&status)
            );
            let Some(job_id) = latest.get("jobId").and_then(Value::as_str) else {
                return latest;
            };
            if !transient || waited >= budget || cancellation.is_cancelled() {
                return latest;
            }
            tokio::time::sleep(backoff).await;
            waited += backoff;
            backoff = (backoff * 2).min(std::time::Duration::from_millis(1200));
            match self
                .bridge
                .call(
                    tool::TERMINAL_POLL,
                    json!({ "jobId": job_id }),
                    cancellation.clone(),
                )
                .await
            {
                Ok(value) => latest = value,
                Err(_) => return latest,
            }
        }
    }

    fn with_discovery_path(mut self, path: std::path::PathBuf) -> Self {
        self.bridge = self.bridge.with_discovery_path(path);
        self
    }

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
        let wait_ms = resolved_wait_ms(&request.name, &mut arguments);
        validate_tool_arguments(&request.name, &arguments)
            .map_err(|error| McpError::invalid_params(error.to_string(), None))?;
        let result = match self
            .bridge
            .call(&request.name, arguments, context.ct.clone())
            .await
        {
            Ok(value) => {
                let value = if wait_ms > 0 {
                    self.await_job_result(value, wait_ms, context.ct.clone())
                        .await
                } else {
                    value
                };
                let validated_name = if wait_ms > 0 {
                    tool::TERMINAL_POLL
                } else {
                    request.name.as_ref()
                };
                match validate_tool_result(validated_name, &value) {
                    Ok(()) => CallToolResult::structured(structured_content(&request.name, value)),
                    Err(error) => CallToolResult::structured_error(json!({
                        "code": "bridge_protocol_error",
                        "message": error.to_string()
                    })),
                }
            }
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
            nyaterm_mcp_protocol::tool_input_schema_for_features(name, targets)
                .ok()?
                .as_object()?
                .clone(),
        );
        item.output_schema =
            nyaterm_mcp_protocol::terminal_job_result_schema_for_features(name, targets)
                .and_then(|schema| schema.as_object().cloned())
                .map(Arc::new);
        if !targets && name == tool::TERMINAL_START {
            item.description = Some("Start a job in an existing scoped terminal session. Defaults to inline_pty; ssh_exec uses an independent channel. Use an idempotencyKey for retries.".into());
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
/// Resolve the forwarder-local wait for terminal_start. The default is
/// context-aware, mirroring `TerminalStartArgs::backend()`: a noninteractive
/// ssh_exec job completes on its own and returns its outcome directly, while
/// an interactive inline_pty session never completes, so waiting by default
/// would only stall the call. waitMs: 0 opts out and longer values are
/// capped. The option is consumed here and never forwarded, so hosts of any
/// version keep working unchanged.
fn resolved_wait_ms(name: &str, arguments: &mut Value) -> u64 {
    if name != tool::TERMINAL_START {
        return 0;
    }
    let requested = arguments
        .as_object_mut()
        .and_then(|object| object.remove("waitMs"))
        .and_then(|value| value.as_u64());
    let noninteractive = match arguments.get("backend").and_then(Value::as_str) {
        Some("ssh_exec") => true,
        Some("inline_pty") => false,
        _ => arguments
            .get("target")
            .and_then(|target| target.get("connectionId"))
            .is_some(),
    };
    match requested {
        Some(0) => 0,
        Some(wait_ms) => wait_ms.min(MAX_WAIT_MS),
        None if noninteractive => DEFAULT_WAIT_MS,
        None => 0,
    }
}

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
            item.output_schema = nyaterm_mcp_protocol::terminal_job_result_schema(definition.tool)
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // A discovery-sourced endpoint is re-resolved on every connection attempt,
    // so a restarted NyaTerm heals the next bridge connection without
    // restarting the forwarder. Explicit environment endpoints stay fixed.
    let server = match resolve_endpoint_source()? {
        EndpointSource::Fixed(endpoint) => NyaTermMcp::new(endpoint),
        EndpointSource::Discovery { endpoint, path } => {
            NyaTermMcp::new(endpoint).with_discovery_path(path)
        }
    }
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
    fn old_host_omits_new_tools_and_parameters_from_individual_tool_schemas() {
        use rmcp::ServerHandler as _;
        let server = NyaTermMcp::new(BridgeEndpoint::for_test(1));
        assert!(server.get_tool(tool::TERMINAL_START).is_none());
        assert!(server.get_tool(tool::TERMINAL_POLL).is_none());
        assert!(server.get_tool(tool::TERMINAL_STOP).is_none());
        let open = server.get_tool(tool::SESSION_OPEN).unwrap();
        let props = open.input_schema["properties"].as_object().unwrap();
        assert!(props.contains_key("connectionId"));
        assert!(!props.contains_key("mode") && !props.contains_key("idempotencyKey"));
        let description = open.description.as_deref().unwrap();
        assert!(!description.contains("reuse") && !description.contains("idempotencyKey"));
        let execute = server.get_tool(tool::TERMINAL_EXECUTE).unwrap();
        let props = execute.input_schema["properties"].as_object().unwrap();
        assert!(props.contains_key("command"));
        assert!(!props.contains_key("returnJob") && !props.contains_key("idempotencyKey"));
        for name in [tool::SFTP_HOME, tool::SFTP_LIST, tool::SFTP_WRITE_TEXT] {
            let item = server.get_tool(name).unwrap();
            assert!(item.input_schema["properties"].get("target").is_none());
            assert!(
                item.input_schema["required"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("sessionId"))
            );
        }
    }
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

        for name in [
            tool::TERMINAL_START,
            tool::TERMINAL_POLL,
            tool::TERMINAL_STOP,
        ] {
            let tool = tools.iter().find(|item| item.name == name).unwrap();
            let value = serde_json::to_value(tool).unwrap();
            assert_eq!(value["outputSchema"]["type"], "object");
            for variant in value["outputSchema"]["anyOf"].as_array().unwrap() {
                assert!(variant["properties"].get("jobId").is_some());
                if name != tool::TERMINAL_START {
                    assert!(variant["properties"].get("processState").is_some());
                }
            }
        }
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

    #[tokio::test]
    async fn initial_tools_list_awaits_feature_negotiation() {
        // Regression test: the bridge connects lazily, so the initial tools/list
        // used to race on_initialized's identify() and hand clients that list
        // tools at startup (and are never notified, since the server does not
        // advertise tools/list_changed) a permanently degraded tool set. A
        // feature-advertising host must therefore be reflected in the FIRST
        // tools/list response, before any tool call.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let bridge_task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (reader, mut writer) = stream.into_split();
            let mut lines = BufReader::new(reader).lines();
            while let Some(line) = lines.next_line().await.unwrap() {
                let request: RpcRequest = serde_json::from_str(&line).unwrap();
                let result = match request.method.as_str() {
                    "auth" => json!({
                        "authenticated": true,
                        "features": ["client_resume", "mcp_lifecycle", "connection_targets"]
                    }),
                    "client.attach" => {
                        assert!(request.params["resumeToken"].is_null());
                        json!({ "ownerId": "test-owner", "resumeToken": "a".repeat(64) })
                    }
                    "client.identify" => json!({ "identified": true }),
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
                        "clientInfo": { "name": "startup-tools-list", "version": "1.0.0" }
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
        // Deliberately no tool call before listing: the very first tools/list
        // must already reflect the negotiated connection targets.
        send_client_message(
            &mut client_writer,
            r#"{ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }"#,
        )
        .await;
        let listed = receive_response(&mut client_lines, 2).await;
        let tools = listed["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), MCP_TOOL_REGISTRY.len());
        let terminal_start = tools
            .iter()
            .find(|item| item["name"] == tool::TERMINAL_START)
            .expect("terminal_start advertised on the initial tools/list");
        assert!(
            terminal_start["inputSchema"]["properties"]
                .as_object()
                .unwrap()
                .contains_key("target")
        );
        let sftp_list = tools
            .iter()
            .find(|item| item["name"] == tool::SFTP_LIST)
            .expect("sftp_list advertised on the initial tools/list");
        assert!(
            sftp_list["inputSchema"]["properties"]
                .as_object()
                .unwrap()
                .contains_key("target")
        );

        // A second listing takes the already-negotiated fast path and must
        // keep advertising the full surface.
        send_client_message(
            &mut client_writer,
            r#"{ "jsonrpc": "2.0", "id": 3, "method": "tools/list" }"#,
        )
        .await;
        let listed_again = receive_response(&mut client_lines, 3).await;
        assert_eq!(
            listed_again["result"]["tools"].as_array().unwrap().len(),
            MCP_TOOL_REGISTRY.len()
        );

        drop(client_writer);
        drop(client_lines);
        tokio::time::timeout(std::time::Duration::from_secs(5), server_task)
            .await
            .expect("MCP service shuts down")
            .expect("MCP service did not panic");
        tokio::time::timeout(std::time::Duration::from_secs(5), bridge_task)
            .await
            .expect("mock host shuts down")
            .expect("mock host did not panic");
    }

    #[tokio::test]
    async fn first_tool_call_awaits_feature_negotiation() {
        // The feature guards in call_tool read connection-negotiated state;
        // the very first tool call (before any tools/list) must not be
        // rejected with unsupported_capability while on_initialized's
        // identify() is still connecting.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let bridge_task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (reader, mut writer) = stream.into_split();
            let mut lines = BufReader::new(reader).lines();
            while let Some(line) = lines.next_line().await.unwrap() {
                let request: RpcRequest = serde_json::from_str(&line).unwrap();
                let result = match request.method.as_str() {
                    "auth" => json!({
                        "authenticated": true,
                        "features": ["client_resume", "mcp_lifecycle", "connection_targets"]
                    }),
                    "client.attach" => {
                        assert!(request.params["resumeToken"].is_null());
                        json!({ "ownerId": "test-owner", "resumeToken": "a".repeat(64) })
                    }
                    "client.identify" => json!({ "identified": true }),
                    "capability.execute" => {
                        let tool = request.params["tool"].as_str().unwrap();
                        assert_eq!(tool, tool::TERMINAL_START);
                        json!({
                            "backend": "ssh_exec",
                            "jobId": "job-1",
                            "status": "starting",
                            "target": { "type": "connection", "connectionId": "c1" }
                        })
                    }
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
                        "clientInfo": { "name": "first-call-test", "version": "1.0.0" }
                    }
                }"#,
        )
        .await;
        receive_response(&mut client_lines, 1).await;
        send_client_message(
            &mut client_writer,
            r#"{ "jsonrpc": "2.0", "method": "notifications/initialized" }"#,
        )
        .await;
        // Deliberately no tools/list first: the very first request is a
        // feature-gated tool call.
        send_client_message(
            &mut client_writer,
            r#"{ "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                    "params": { "name": "terminal_start", "arguments": {
                        "command": "echo hi",
                        "backend": "ssh_exec",
                        "target": { "type": "connection", "connectionId": "c1" },
                        "waitMs": 0
                    } } }"#,
        )
        .await;
        let called = receive_response(&mut client_lines, 2).await;
        let structured = &called["result"]["structuredContent"];
        assert_ne!(
            structured.get("code").and_then(Value::as_str),
            Some("unsupported_capability"),
            "the first call must not race the negotiation: {called}"
        );
        assert_eq!(structured["status"], "starting");

        drop(client_writer);
        drop(client_lines);
        tokio::time::timeout(std::time::Duration::from_secs(5), server_task)
            .await
            .expect("MCP service shuts down")
            .expect("MCP service did not panic");
        tokio::time::timeout(std::time::Duration::from_secs(5), bridge_task)
            .await
            .expect("mock host shuts down")
            .expect("mock host did not panic");
    }
    #[tokio::test]
    async fn terminal_start_wait_ms_returns_completed_in_one_call() {
        // waitMs is consumed by the forwarder: the client makes a single
        // tools/call and receives the job's final state, while the forwarder
        // polls the bridge internally. The host never sees the option.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let bridge_task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (reader, mut writer) = stream.into_split();
            let mut lines = BufReader::new(reader).lines();
            let mut polls = 0;
            while let Some(line) = lines.next_line().await.unwrap() {
                let request: RpcRequest = serde_json::from_str(&line).unwrap();
                let result = match request.method.as_str() {
                    "auth" => json!({
                        "authenticated": true,
                        "features": ["mcp_lifecycle", "connection_targets"]
                    }),
                    "client.identify" => json!({ "identified": true }),
                    "capability.execute" => match request.params["tool"].as_str().unwrap() {
                        tool::TERMINAL_START => {
                            assert!(
                                request.params["arguments"].get("waitMs").is_none(),
                                "waitMs must not be forwarded to the host"
                            );
                            json!({
                                "backend": "ssh_exec",
                                "jobId": "job-1",
                                "status": "starting",
                                "target": { "type": "connection", "connectionId": "c1" }
                            })
                        }
                        tool::TERMINAL_POLL => {
                            polls += 1;
                            if polls == 1 {
                                json!({
                                    "backend": "ssh_exec", "jobId": "job-1",
                                    "status": "running",
                                    "target": { "type": "connection", "connectionId": "c1" },
                                    "offset": 0, "baseOffset": 0, "nextOffset": 0,
                                    "output": "", "eof": false, "outputComplete": false,
                                    "sourceTruncated": false, "droppedBeforeOffset": false,
                                    "totalBytes": 0, "processState": "running",
                                    "stopState": "not_requested", "startedAtMs": 1
                                })
                            } else {
                                json!({
                                    "backend": "ssh_exec", "jobId": "job-1",
                                    "status": "completed", "exitCode": 0,
                                    "target": { "type": "connection", "connectionId": "c1" },
                                    "offset": 0, "baseOffset": 0, "nextOffset": 5,
                                    "output": "done!", "eof": true, "outputComplete": true,
                                    "sourceTruncated": false, "droppedBeforeOffset": false,
                                    "totalBytes": 5, "processState": "exit_confirmed",
                                    "stopState": "not_requested", "startedAtMs": 1, "endedAtMs": 2
                                })
                            }
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
                        "clientInfo": { "name": "waitms-test", "version": "1.0.0" }
                    }
                }"#,
        )
        .await;
        receive_response(&mut client_lines, 1).await;
        send_client_message(
            &mut client_writer,
            r#"{ "jsonrpc": "2.0", "method": "notifications/initialized" }"#,
        )
        .await;
        send_client_message(
            &mut client_writer,
            r#"{ "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                    "params": { "name": "terminal_start", "arguments": {
                        "command": "echo done!",
                        "backend": "ssh_exec",
                        "target": { "type": "connection", "connectionId": "c1" },
                        "waitMs": 5000
                    } } }"#,
        )
        .await;
        let completed = receive_response(&mut client_lines, 2).await;
        let structured = &completed["result"]["structuredContent"];
        assert_eq!(structured["status"], "completed");
        assert_eq!(structured["exitCode"], 0);
        assert_eq!(structured["output"], "done!");

        drop(client_writer);
        drop(client_lines);
        server_task.abort();
        bridge_task.abort();
    }

    #[tokio::test]
    async fn terminal_start_waits_by_default_and_wait_ms_zero_opts_out() {
        // A plain terminal_start must return the command's outcome directly
        // (zero-decision design: models need no polling loop and no prior
        // knowledge), while waitMs: 0 restores the immediate async result.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let bridge_task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (reader, mut writer) = stream.into_split();
            let mut lines = BufReader::new(reader).lines();
            let mut starts = 0;
            while let Some(line) = lines.next_line().await.unwrap() {
                let request: RpcRequest = serde_json::from_str(&line).unwrap();
                let result = match request.method.as_str() {
                    "auth" => json!({
                        "authenticated": true,
                        "features": ["mcp_lifecycle", "connection_targets"]
                    }),
                    "client.identify" => json!({ "identified": true }),
                    "capability.execute" => match request.params["tool"].as_str().unwrap() {
                        tool::TERMINAL_START => {
                            assert!(
                                request.params["arguments"].get("waitMs").is_none(),
                                "waitMs must not be forwarded to the host"
                            );
                            starts += 1;
                            json!({
                                "backend": "ssh_exec",
                                "jobId": format!("job-{starts}"),
                                "status": "starting",
                                "target": { "type": "connection", "connectionId": "c1" }
                            })
                        }
                        tool::TERMINAL_POLL => {
                            json!({
                                "backend": "ssh_exec", "jobId": request.params["arguments"]["jobId"],
                                "status": "completed", "exitCode": 0,
                                "target": { "type": "connection", "connectionId": "c1" },
                                "offset": 0, "baseOffset": 0, "nextOffset": 0,
                                "output": "ok", "eof": true, "outputComplete": true,
                                "sourceTruncated": false, "droppedBeforeOffset": false,
                                "totalBytes": 2, "processState": "exit_confirmed",
                                "stopState": "not_requested", "startedAtMs": 1, "endedAtMs": 2
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
                        "clientInfo": { "name": "default-wait-test", "version": "1.0.0" }
                    }
                }"#,
        )
        .await;
        receive_response(&mut client_lines, 1).await;
        send_client_message(
            &mut client_writer,
            r#"{ "jsonrpc": "2.0", "method": "notifications/initialized" }"#,
        )
        .await;

        // Plain call: no waitMs, outcome must arrive without any poll from
        // the client (the forwarder waits internally).
        send_client_message(
            &mut client_writer,
            r#"{ "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                    "params": { "name": "terminal_start", "arguments": {
                        "command": "echo ok",
                        "backend": "ssh_exec",
                        "target": { "type": "connection", "connectionId": "c1" }
                    } } }"#,
        )
        .await;
        let defaulted = receive_response(&mut client_lines, 2).await;
        assert_eq!(
            defaulted["result"]["structuredContent"]["status"],
            "completed"
        );
        assert_eq!(defaulted["result"]["structuredContent"]["output"], "ok");

        // Explicit opt-out: the immediate async result must come back without
        // the forwarder polling (the mock's poll answer would otherwise turn
        // this into completed).
        send_client_message(
            &mut client_writer,
            r#"{ "jsonrpc": "2.0", "id": 3, "method": "tools/call",
                    "params": { "name": "terminal_start", "arguments": {
                        "command": "sleep 300",
                        "backend": "ssh_exec",
                        "target": { "type": "connection", "connectionId": "c1" },
                        "waitMs": 0
                    } } }"#,
        )
        .await;
        let opted_out = receive_response(&mut client_lines, 3).await;
        assert_eq!(
            opted_out["result"]["structuredContent"]["status"],
            "starting"
        );
        assert_eq!(opted_out["result"]["structuredContent"]["jobId"], "job-2");

        // Interactive session start (inline_pty) never completes, so the
        // context-aware default must not wait: the result returns immediately.
        send_client_message(
            &mut client_writer,
            r#"{ "jsonrpc": "2.0", "id": 4, "method": "tools/call",
                    "params": { "name": "terminal_start", "arguments": {
                        "command": "top",
                        "sessionId": "session-1"
                    } } }"#,
        )
        .await;
        let interactive = receive_response(&mut client_lines, 4).await;
        assert_eq!(
            interactive["result"]["structuredContent"]["status"],
            "starting"
        );
        assert_eq!(interactive["result"]["structuredContent"]["jobId"], "job-3");

        drop(client_writer);
        drop(client_lines);
        server_task.abort();
        bridge_task.abort();
    }
}
