//! Application adapter for requests received by the UI-independent host.
use crate::features::capability_runtime::{CapabilityCaller, CapabilityRequest};
use nyaterm_mcp_host::server::McpHostRequest;
pub(in crate::features) use nyaterm_mcp_host::server::{McpHostEvent, McpHostRuntime, rpc_failure};

impl From<McpHostRequest> for CapabilityRequest {
    fn from(request: McpHostRequest) -> Self {
        Self {
            extended_results: request.extended_results,
            resolved_session_id: None,
            connection_id: request.connection_id,
            request_id: request.request_id,
            generation: request.generation,
            client: request.client,
            permission_mode: request.permission_mode,
            scope: request.scope,
            tool: request.tool,
            arguments: request.arguments,
            cancellation: request.cancellation,
            reply: request.reply,
            caller: CapabilityCaller::Mcp,
            approved: false,
            approval_decision: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{McpHostEvent, McpHostRuntime, rpc_failure};
    use futures::StreamExt;
    use nyaterm_core::{AiPermissionMode, CapabilityScope};
    use nyaterm_mcp_protocol::{AuthParams, MAX_INLINE_OUTPUT_BYTES, RpcRequest, RpcResponse};
    use serde_json::json;
    use std::net::Ipv4Addr;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpStream;
    async fn rpc(
        reader: &mut tokio::io::Lines<BufReader<tokio::net::tcp::OwnedReadHalf>>,
        writer: &mut tokio::net::tcp::OwnedWriteHalf,
        request: RpcRequest,
    ) -> RpcResponse {
        let mut bytes = serde_json::to_vec(&request).unwrap();
        bytes.push(b'\n');
        writer.write_all(&bytes).await.unwrap();
        serde_json::from_str(&reader.next_line().await.unwrap().unwrap()).unwrap()
    }

    #[tokio::test]
    async fn host_authenticates_and_forwards_common_runtime_results_unchanged() {
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        let runtime = McpHostRuntime::start(
            AiPermissionMode::Auto,
            CapabilityScope::AllSessions,
            tx,
            None,
        )
        .unwrap();
        let endpoint = runtime.endpoint();
        let stream = TcpStream::connect((Ipv4Addr::LOCALHOST, endpoint.port))
            .await
            .unwrap();
        let (reader, mut writer) = stream.into_split();
        let mut lines = BufReader::new(reader).lines();
        let auth = rpc(
            &mut lines,
            &mut writer,
            RpcRequest {
                id: 1,
                method: "auth".into(),
                params: serde_json::to_value(AuthParams {
                    token: endpoint.token.clone(),
                    generation: endpoint.generation.clone(),
                })
                .unwrap(),
            },
        )
        .await;
        assert_eq!(auth.result.unwrap()["authenticated"], true);

        let write = tokio::spawn(async move {
            let response = rpc(
                &mut lines,
                &mut writer,
                RpcRequest {
                    id: 2,
                    method: "capability.execute".into(),
                    params: json!({ "tool": "sftp_delete", "arguments": { "sessionId": "s", "path": "/tmp/x" } }),
                },
            )
            .await;
            (response, lines, writer)
        });
        let McpHostEvent::Execute(request) = rx.next().await.unwrap() else {
            panic!("expected execute request");
        };
        let request: crate::features::capability_runtime::CapabilityRequest = (*request).into();
        assert!(!request.approved);
        assert!(request.approval_decision.is_none());
        assert!(request.resolved_session_id.is_none());
        assert!(matches!(
            request.caller,
            crate::features::capability_runtime::CapabilityCaller::Mcp
        ));
        assert_eq!(request.tool, "sftp_delete");
        request
            .reply
            .send(Err(rpc_failure("permission_denied", "denied by policy")))
            .unwrap();
        let (write_response, mut lines, mut writer) = write.await.unwrap();
        assert_eq!(write_response.error.unwrap().code, "permission_denied");

        let call = tokio::spawn(async move {
            rpc(
                &mut lines,
                &mut writer,
                RpcRequest {
                    id: 3,
                    method: "capability.execute".into(),
                    params: json!({ "tool": "connection_list", "arguments": {} }),
                },
            )
            .await
        });
        let McpHostEvent::Execute(request) = rx.next().await.unwrap() else {
            panic!("expected execute request");
        };
        let value = json!({"connections":[{"id":"c","name":"x".repeat(MAX_INLINE_OUTPUT_BYTES),"type":"ssh","groupPath":[]}]});
        let mut capabilities =
            crate::features::capability_runtime::CapabilityRuntimeState::default();
        let result = capabilities
            .protect_output(&request.connection_id, "connection_list", value)
            .unwrap();
        request.reply.send(Ok(result)).unwrap();
        let response = call.await.unwrap().result.unwrap();
        assert_eq!(response["truncated"], true);
        assert!(response["outputId"].as_str().unwrap().starts_with("out_"));
        drop(runtime);
    }
}
