use std::path::PathBuf;
use std::sync::Arc;

use nyaterm_mcp_protocol::{
    AuthParams, CapabilityExecuteParams, ClientIdentifyParams, DiscoveryDocument, PROTOCOL_VERSION,
    RpcError, RpcRequest, RpcResponse,
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct BridgeEndpoint {
    host: String,
    port: u16,
    token: String,
    generation: String,
}

impl BridgeEndpoint {
    #[cfg(test)]
    pub(crate) fn for_test(port: u16) -> Self {
        Self {
            host: "127.0.0.1".into(),
            port,
            token: "test-token".into(),
            generation: "test-generation".into(),
        }
    }
}

struct Connection {
    reader: BufReader<tokio::net::tcp::OwnedReadHalf>,
    writer: tokio::net::tcp::OwnedWriteHalf,
    next_id: u64,
}

#[derive(Clone)]
pub struct BridgeClient {
    endpoint: BridgeEndpoint,
    connection: Arc<Mutex<Option<Connection>>>,
    identity: Arc<Mutex<Option<ClientIdentifyParams>>>,
}

impl BridgeClient {
    pub fn new(endpoint: BridgeEndpoint) -> Self {
        Self {
            endpoint,
            connection: Arc::new(Mutex::new(None)),
            identity: Arc::new(Mutex::new(None)),
        }
    }

    pub async fn identify(&self, name: String, version: Option<String>) {
        let identity = ClientIdentifyParams { name, version };
        *self.identity.lock().await = Some(identity.clone());
        let params = serde_json::to_value(identity).unwrap_or_default();
        let _ = self.rpc("client.identify", params).await;
    }

    pub async fn call(
        &self,
        name: &str,
        arguments: Value,
        cancellation: CancellationToken,
    ) -> Result<Value, RpcError> {
        let request_id = uuid::Uuid::new_v4().to_string();
        let params = serde_json::to_value(CapabilityExecuteParams {
            request_id: Some(request_id.clone()),
            tool: name.to_string(),
            arguments,
        })
        .map_err(|error| bridge_error("invalid_argument", &error.to_string()))?;
        let mut guard = self.connection.lock().await;
        self.ensure_connection(&mut guard, true).await?;
        let connection = guard.as_mut().unwrap();
        let result = tokio::select! {
            _ = cancellation.cancelled() => {
                let endpoint = self.endpoint.clone();
                tokio::spawn(async move { let _ = cancel_request(&endpoint, &request_id).await; });
                *guard = None;
                return Err(bridge_error("cancelled", "The MCP tool call was cancelled."));
            }
            result = connection_rpc(connection, "capability.execute", params) => result,
        };
        finish_rpc(&mut guard, result)
    }

    async fn rpc(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        let mut guard = self.connection.lock().await;
        self.ensure_connection(&mut guard, method != "client.identify")
            .await?;
        let connection = guard.as_mut().unwrap();
        let result = connection_rpc(connection, method, params).await;
        finish_rpc(&mut guard, result)
    }

    async fn ensure_connection(
        &self,
        guard: &mut Option<Connection>,
        replay_identity: bool,
    ) -> Result<(), RpcError> {
        if guard.is_some() {
            return Ok(());
        }
        let mut connection = connect(&self.endpoint).await.map_err(io_error)?;
        if replay_identity && let Some(identity) = self.identity.lock().await.clone() {
            let params = serde_json::to_value(identity)
                .map_err(|error| bridge_error("invalid_argument", &error.to_string()))?;
            match connection_rpc(&mut connection, "client.identify", params).await {
                Ok(_) => {}
                Err(ConnectionRpcError::Remote(error)) => return Err(error),
                Err(ConnectionRpcError::Disconnected(error)) => return Err(error),
            }
        }
        *guard = Some(connection);
        Ok(())
    }
}

enum ConnectionRpcError {
    Remote(RpcError),
    Disconnected(RpcError),
}

fn finish_rpc(
    guard: &mut Option<Connection>,
    result: Result<Value, ConnectionRpcError>,
) -> Result<Value, RpcError> {
    match result {
        Ok(value) => Ok(value),
        Err(ConnectionRpcError::Remote(error)) => Err(error),
        Err(ConnectionRpcError::Disconnected(error)) => {
            *guard = None;
            Err(error)
        }
    }
}

async fn connection_rpc(
    connection: &mut Connection,
    method: &str,
    params: Value,
) -> Result<Value, ConnectionRpcError> {
    let id = connection.next_id;
    connection.next_id += 1;
    write_request(
        connection,
        RpcRequest {
            id,
            method: method.into(),
            params,
        },
    )
    .await
    .map_err(|error| ConnectionRpcError::Disconnected(io_error(error)))?;
    let response = read_response(connection)
        .await
        .map_err(|error| ConnectionRpcError::Disconnected(io_error(error)))?;
    if response.id != id {
        return Err(ConnectionRpcError::Disconnected(bridge_error(
            "bridge_disconnected",
            "MCP bridge response ID mismatch.",
        )));
    }
    match (response.result, response.error) {
        (Some(value), None) => Ok(value),
        (_, Some(error)) => Err(ConnectionRpcError::Remote(error)),
        _ => Err(ConnectionRpcError::Disconnected(bridge_error(
            "bridge_disconnected",
            "MCP bridge returned an empty response.",
        ))),
    }
}

async fn connect(endpoint: &BridgeEndpoint) -> std::io::Result<Connection> {
    let stream = TcpStream::connect((endpoint.host.as_str(), endpoint.port)).await?;
    let (read, write) = stream.into_split();
    let mut connection = Connection {
        reader: BufReader::new(read),
        writer: write,
        next_id: 1,
    };
    let params = serde_json::to_value(AuthParams {
        token: endpoint.token.clone(),
        generation: endpoint.generation.clone(),
    })
    .map_err(std::io::Error::other)?;
    connection_rpc(&mut connection, "auth", params)
        .await
        .map_err(|error| match error {
            ConnectionRpcError::Remote(error) | ConnectionRpcError::Disconnected(error) => {
                std::io::Error::new(std::io::ErrorKind::PermissionDenied, error.message)
            }
        })?;
    Ok(connection)
}

async fn cancel_request(endpoint: &BridgeEndpoint, request_id: &str) -> std::io::Result<()> {
    let mut connection = connect(endpoint).await?;
    connection_rpc(
        &mut connection,
        "request.cancel",
        json!({ "requestId": request_id }),
    )
    .await
    .map_err(|error| match error {
        ConnectionRpcError::Remote(error) | ConnectionRpcError::Disconnected(error) => {
            std::io::Error::other(error.message)
        }
    })?;
    Ok(())
}

async fn write_request(connection: &mut Connection, request: RpcRequest) -> std::io::Result<()> {
    let mut bytes = serde_json::to_vec(&request).map_err(std::io::Error::other)?;
    bytes.push(b'\n');
    connection.writer.write_all(&bytes).await
}

async fn read_response(connection: &mut Connection) -> std::io::Result<RpcResponse> {
    let mut line = String::new();
    if connection.reader.read_line(&mut line).await? == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "NyaTerm MCP bridge disconnected",
        ));
    }
    serde_json::from_str(line.trim_end()).map_err(std::io::Error::other)
}

pub fn endpoint_from_environment_or_discovery() -> Result<BridgeEndpoint, Box<dyn std::error::Error>>
{
    let ephemeral = std::env::var("NYATERM_MCP_EPHEMERAL").as_deref() == Ok("1");
    let host = std::env::var("NYATERM_MCP_HOST").ok();
    let port = std::env::var("NYATERM_MCP_PORT")
        .ok()
        .and_then(|value| value.parse().ok());
    let token = std::env::var("NYATERM_MCP_TOKEN").ok();
    let generation = std::env::var("NYATERM_MCP_GENERATION").ok();
    if let (Some(host), Some(port), Some(token), Some(generation)) = (host, port, token, generation)
    {
        if host != "127.0.0.1" {
            return Err("NyaTerm MCP bridge host must be 127.0.0.1".into());
        }
        return Ok(BridgeEndpoint {
            host,
            port,
            token,
            generation,
        });
    }
    if ephemeral {
        return Err("NyaTerm ephemeral MCP credential is incomplete or unavailable".into());
    }
    let document: DiscoveryDocument = serde_json::from_slice(&std::fs::read(discovery_path()?)?)?;
    if document.version != PROTOCOL_VERSION || document.host != "127.0.0.1" {
        return Err("Unsupported or unsafe NyaTerm MCP discovery document".into());
    }
    Ok(BridgeEndpoint {
        host: document.host,
        port: document.port,
        token: document.token,
        generation: document.generation,
    })
}

fn discovery_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
    if let Some(path) = std::env::var_os("NYATERM_MCP_DISCOVERY") {
        return Ok(path.into());
    }
    let executable = std::env::current_exe()?;
    let directory = executable
        .parent()
        .ok_or("Cannot resolve sidecar directory")?;
    if directory.join("portable.flag").is_file() {
        return Ok(directory
            .join("data")
            .join("config")
            .join("mcp")
            .join("discovery.json"));
    }
    Ok(dirs::home_dir()
        .ok_or("Cannot resolve home directory")?
        .join(".nyaterm")
        .join("mcp")
        .join("discovery.json"))
}

fn io_error(error: std::io::Error) -> RpcError {
    bridge_error("bridge_disconnected", &error.to_string())
}
fn bridge_error(code: &str, message: &str) -> RpcError {
    RpcError {
        code: code.into(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tokio::net::TcpListener;

    use super::*;

    async fn request(
        lines: &mut tokio::io::Lines<BufReader<tokio::net::tcp::OwnedReadHalf>>,
    ) -> RpcRequest {
        serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap()
    }

    async fn respond(writer: &mut tokio::net::tcp::OwnedWriteHalf, id: u64, result: Value) {
        let mut bytes = serde_json::to_vec(&RpcResponse {
            id,
            result: Some(result),
            error: None,
        })
        .unwrap();
        bytes.push(b'\n');
        writer.write_all(&bytes).await.unwrap();
    }

    async fn authenticate(
        stream: TcpStream,
        auth_count: &AtomicUsize,
    ) -> (
        tokio::io::Lines<BufReader<tokio::net::tcp::OwnedReadHalf>>,
        tokio::net::tcp::OwnedWriteHalf,
    ) {
        let (reader, mut writer) = stream.into_split();
        let mut lines = BufReader::new(reader).lines();
        let auth = request(&mut lines).await;
        assert_eq!(auth.method, "auth");
        assert_eq!(auth.params["token"], "test-token");
        auth_count.fetch_add(1, Ordering::SeqCst);
        respond(&mut writer, auth.id, json!({ "authenticated": true })).await;
        (lines, writer)
    }

    #[tokio::test]
    async fn disconnect_invalidates_and_next_call_reauthenticates_and_identifies() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let auth_count = Arc::new(AtomicUsize::new(0));
        let server_count = auth_count.clone();
        let server = tokio::spawn(async move {
            let (first, _) = listener.accept().await.unwrap();
            let (mut lines, mut writer) = authenticate(first, &server_count).await;
            let identify = request(&mut lines).await;
            assert_eq!(identify.method, "client.identify");
            respond(&mut writer, identify.id, json!({ "identified": true })).await;
            let call = request(&mut lines).await;
            assert_eq!(call.method, "capability.execute");
            respond(&mut writer, call.id, json!({ "value": "first" })).await;
            drop(writer);

            let (second, _) = listener.accept().await.unwrap();
            let (mut lines, mut writer) = authenticate(second, &server_count).await;
            let identify = request(&mut lines).await;
            assert_eq!(identify.method, "client.identify");
            respond(&mut writer, identify.id, json!({ "identified": true })).await;
            let call = request(&mut lines).await;
            respond(&mut writer, call.id, json!({ "value": "recovered" })).await;
        });

        let client = BridgeClient::new(BridgeEndpoint::for_test(port));
        client
            .identify("bridge-test".into(), Some("1.0".into()))
            .await;
        let first = client
            .call("get_environment", json!({}), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(first["value"], "first");

        let disconnected = client
            .call("get_environment", json!({}), CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(disconnected.code, "bridge_disconnected");

        let recovered = client
            .call("get_environment", json!({}), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(recovered["value"], "recovered");
        server.await.unwrap();
        assert_eq!(auth_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn response_id_mismatch_invalidates_the_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let auth_count = Arc::new(AtomicUsize::new(0));
        let server_count = auth_count.clone();
        let server = tokio::spawn(async move {
            let (first, _) = listener.accept().await.unwrap();
            let (mut lines, mut writer) = authenticate(first, &server_count).await;
            let call = request(&mut lines).await;
            respond(&mut writer, call.id + 1, json!({ "wrong": true })).await;
            drop(writer);

            let (second, _) = listener.accept().await.unwrap();
            let (mut lines, mut writer) = authenticate(second, &server_count).await;
            let call = request(&mut lines).await;
            respond(&mut writer, call.id, json!({ "recovered": true })).await;
        });

        let client = BridgeClient::new(BridgeEndpoint::for_test(port));
        let mismatch = client
            .call("get_environment", json!({}), CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(mismatch.code, "bridge_disconnected");
        let recovered = client
            .call("get_environment", json!({}), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(recovered["recovered"], true);
        server.await.unwrap();
        assert_eq!(auth_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn invalid_response_invalidates_the_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let auth_count = Arc::new(AtomicUsize::new(0));
        let server_count = auth_count.clone();
        let server = tokio::spawn(async move {
            let (first, _) = listener.accept().await.unwrap();
            let (mut lines, mut writer) = authenticate(first, &server_count).await;
            let _ = request(&mut lines).await;
            writer.write_all(b"not-json\n").await.unwrap();
            drop(writer);

            let (second, _) = listener.accept().await.unwrap();
            let (mut lines, mut writer) = authenticate(second, &server_count).await;
            let call = request(&mut lines).await;
            respond(&mut writer, call.id, json!({ "recovered": true })).await;
        });

        let client = BridgeClient::new(BridgeEndpoint::for_test(port));
        let invalid = client
            .call("get_environment", json!({}), CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(invalid.code, "bridge_disconnected");
        assert!(
            client
                .call("get_environment", json!({}), CancellationToken::new())
                .await
                .unwrap()["recovered"]
                .as_bool()
                .unwrap()
        );
        server.await.unwrap();
        assert_eq!(auth_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn empty_response_invalidates_the_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let auth_count = Arc::new(AtomicUsize::new(0));
        let server_count = auth_count.clone();
        let server = tokio::spawn(async move {
            let (first, _) = listener.accept().await.unwrap();
            let (mut lines, mut writer) = authenticate(first, &server_count).await;
            let call = request(&mut lines).await;
            let mut bytes = serde_json::to_vec(&RpcResponse {
                id: call.id,
                result: None,
                error: None,
            })
            .unwrap();
            bytes.push(b'\n');
            writer.write_all(&bytes).await.unwrap();
            drop(writer);

            let (second, _) = listener.accept().await.unwrap();
            let (mut lines, mut writer) = authenticate(second, &server_count).await;
            let call = request(&mut lines).await;
            respond(&mut writer, call.id, json!({ "recovered": true })).await;
        });

        let client = BridgeClient::new(BridgeEndpoint::for_test(port));
        let empty = client
            .call("get_environment", json!({}), CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(empty.code, "bridge_disconnected");
        assert_eq!(
            client
                .call("get_environment", json!({}), CancellationToken::new())
                .await
                .unwrap()["recovered"],
            true
        );
        server.await.unwrap();
        assert_eq!(auth_count.load(Ordering::SeqCst), 2);
    }
}
