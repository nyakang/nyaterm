mod bridge;
mod terminal_start;

mod handler;
mod http_server;
#[cfg(test)]
use bridge::BridgeEndpoint;
use bridge::{EndpointSource, resolve_endpoint_source};
use handler::NyaTermMcp;
#[cfg(test)]
use handler::{build_tools, structured_content};
use rmcp::ServiceExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args == ["--transport", "streamable-http", "--managed"] {
        return http_server::run_managed().await;
    }
    if !args.is_empty() && args != ["--transport", "stdio"] {
        return Err(
            "Usage: nyaterm-mcp [--transport stdio | --transport streamable-http --managed]".into(),
        );
    }
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
mod schema_tests;

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
}
