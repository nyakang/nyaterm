use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use nyaterm_mcp_protocol::{
    AuthParams, CapabilityExecuteParams, ClientIdentifyParams, DiscoveryDocument,
    MAX_RPC_LINE_BYTES, PROTOCOL_VERSION, RequestCancelParams, RpcError, RpcRequest, RpcResponse,
};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

const DEFAULT_RPC_TIMEOUT: Duration = Duration::from_secs(305);

#[derive(Clone)]
pub struct BridgeEndpoint {
    host: String,
    port: u16,
    token: String,
    generation: String,
    rpc_timeout: Duration,
}

impl BridgeEndpoint {
    #[cfg(test)]
    pub(crate) fn for_test(port: u16) -> Self {
        Self {
            host: "127.0.0.1".into(),
            port,
            token: "test-token".into(),
            generation: "test-generation".into(),
            rpc_timeout: Duration::from_secs(2),
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test_with_timeout(port: u16, rpc_timeout: Duration) -> Self {
        Self {
            rpc_timeout,
            ..Self::for_test(port)
        }
    }
}

struct Connection {
    client_resume: bool,
    lifecycle: bool,
    reader: BufReader<tokio::net::tcp::OwnedReadHalf>,
    writer: tokio::net::tcp::OwnedWriteHalf,
    next_id: u64,
    rpc_timeout: Duration,
}

#[derive(Clone)]
pub struct BridgeClient {
    endpoint: BridgeEndpoint,
    connection: Arc<Mutex<Option<Connection>>>,
    identity: Arc<Mutex<Option<ClientIdentifyParams>>>,
    resume_token: Arc<Mutex<Option<nyaterm_core::SecretString>>>,
    lifecycle: Arc<AtomicBool>,
}

impl BridgeClient {
    pub fn new(endpoint: BridgeEndpoint) -> Self {
        Self {
            endpoint,
            connection: Arc::new(Mutex::new(None)),
            identity: Arc::new(Mutex::new(None)),
            resume_token: Arc::new(Mutex::new(None)),
            lifecycle: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn supports_lifecycle(&self) -> bool {
        self.lifecycle.load(Ordering::Acquire)
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
        if cancellation.is_cancelled() {
            return Err(bridge_error(
                "cancelled",
                "The tool call was cancelled before submission.",
            ));
        }
        let request_id = uuid::Uuid::new_v4().to_string();
        let params = serde_json::to_value(CapabilityExecuteParams {
            request_id: Some(request_id.clone()),
            tool: name.to_string(),
            arguments,
        })
        .map_err(|error| bridge_error("invalid_argument", &error.to_string()))?;
        let mut guard = self.connection.lock().await;
        if cancellation.is_cancelled() {
            return Err(bridge_error(
                "cancelled",
                "The tool call was cancelled before submission.",
            ));
        }
        self.ensure_connection(&mut guard, true).await?;
        let connection = guard.as_mut().unwrap();
        let result = tokio::select! {
            _ = cancellation.cancelled() => {
                let endpoint = self.endpoint.clone();
                let resume = self.resume_token.lock().await.clone();
                tokio::spawn(async move { let _ = cancel_request(&endpoint, &request_id, resume.as_ref()).await; });
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
        self.lifecycle
            .store(connection.lifecycle, Ordering::Release);
        if connection.client_resume {
            let resume = self.resume_token.lock().await.clone();
            let token = attach_owner(&mut connection, resume.as_ref()).await?;
            *self.resume_token.lock().await = Some(token);
        }

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
    let timeout = connection.rpc_timeout;
    match tokio::time::timeout(timeout, connection_rpc_inner(connection, method, params)).await {
        Ok(result) => result,
        Err(_) => Err(ConnectionRpcError::Disconnected(bridge_error(
            "bridge_timeout",
            "NyaTerm MCP bridge request timed out.",
        ))),
    }
}

async fn connection_rpc_inner(
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
    if let Err(message) = response.validate_envelope() {
        return Err(ConnectionRpcError::Disconnected(bridge_error(
            "bridge_disconnected",
            message,
        )));
    }
    match (response.result, response.error) {
        (Some(value), None) => Ok(value),
        (None, Some(error)) => Err(ConnectionRpcError::Remote(error)),
        _ => unreachable!("validated RPC response envelope"),
    }
}

async fn connect(endpoint: &BridgeEndpoint) -> std::io::Result<Connection> {
    let stream = TcpStream::connect((endpoint.host.as_str(), endpoint.port)).await?;
    let (read, write) = stream.into_split();
    let mut connection = Connection {
        client_resume: false,
        lifecycle: false,
        reader: BufReader::new(read),
        writer: write,
        next_id: 1,
        rpc_timeout: endpoint.rpc_timeout,
    };
    let params = serde_json::to_value(AuthParams {
        token: endpoint.token.clone(),
        generation: endpoint.generation.clone(),
    })
    .map_err(std::io::Error::other)?;
    let authenticated = connection_rpc(&mut connection, "auth", params)
        .await
        .map_err(|error| match error {
            ConnectionRpcError::Remote(error) | ConnectionRpcError::Disconnected(error) => {
                std::io::Error::new(std::io::ErrorKind::PermissionDenied, error.message)
            }
        })?;
    connection.client_resume = authenticated["features"]
        .as_array()
        .is_some_and(|f| f.iter().any(|v| v == "client_resume"));
    connection.lifecycle = authenticated["features"]
        .as_array()
        .is_some_and(|f| f.iter().any(|v| v == "mcp_lifecycle"));
    Ok(connection)
}

async fn attach_owner(
    connection: &mut Connection,
    resume: Option<&nyaterm_core::SecretString>,
) -> Result<nyaterm_core::SecretString, RpcError> {
    let params = serde_json::json!({ "resumeToken": resume.map(|s| s.expose_secret()) });
    let result = connection_rpc(connection, "client.attach", params)
        .await
        .map_err(|e| match e {
            ConnectionRpcError::Remote(e) | ConnectionRpcError::Disconnected(e) => e,
        })?;
    let token = result["resumeToken"]
        .as_str()
        .filter(|token| token.len() == 64)
        .ok_or_else(|| {
            bridge_error("bridge_protocol_error", "Invalid client recovery response.")
        })?;
    Ok(nyaterm_core::SecretString::new(token.to_string()))
}

async fn cancel_request(
    endpoint: &BridgeEndpoint,
    request_id: &str,
    resume: Option<&nyaterm_core::SecretString>,
) -> std::io::Result<()> {
    let mut connection = connect(endpoint).await?;
    if connection.client_resume {
        attach_owner(&mut connection, resume)
            .await
            .map_err(|e| std::io::Error::other(e.code))?;
    }
    let params = serde_json::to_value(RequestCancelParams {
        request_id: request_id.to_string(),
    })
    .map_err(std::io::Error::other)?;
    connection_rpc(&mut connection, "request.cancel", params)
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
    if bytes.len().saturating_add(1) > MAX_RPC_LINE_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "MCP bridge request line is too large",
        ));
    }
    bytes.push(b'\n');
    connection.writer.write_all(&bytes).await
}

async fn read_response(connection: &mut Connection) -> std::io::Result<RpcResponse> {
    let line = read_rpc_line(&mut connection.reader)
        .await?
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "NyaTerm MCP bridge disconnected",
            )
        })?;
    serde_json::from_slice(&line).map_err(std::io::Error::other)
}

async fn read_rpc_line<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut BufReader<R>,
) -> std::io::Result<Option<Vec<u8>>> {
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            if line.is_empty() {
                return Ok(None);
            }
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "MCP bridge response line is not newline terminated",
            ));
        }
        let take = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |index| index + 1);
        if line.len().saturating_add(take) > MAX_RPC_LINE_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "MCP bridge response line is too large",
            ));
        }
        line.extend_from_slice(&available[..take]);
        reader.consume(take);
        if line.last() == Some(&b'\n') {
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            return Ok(Some(line));
        }
    }
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
            rpc_timeout: DEFAULT_RPC_TIMEOUT,
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
        rpc_timeout: DEFAULT_RPC_TIMEOUT,
    })
}

fn discovery_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
    if let Some(path) = std::env::var_os("NYATERM_MCP_DISCOVERY") {
        return Ok(path.into());
    }
    discovery_path_from(
        &std::env::current_exe()?,
        dirs::home_dir().as_deref(),
        nyaterm_core::app_identity::AppFlavor::current(),
    )
}

fn discovery_path_from(
    executable: &std::path::Path,
    home: Option<&std::path::Path>,
    flavor: nyaterm_core::app_identity::AppFlavor,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let directory = executable
        .parent()
        .ok_or("Cannot resolve sidecar directory")?;
    let config = if ["nyaterm-portable", "portable.flag"]
        .into_iter()
        .any(|marker| directory.join(marker).is_file())
    {
        directory.join("data/config")
    } else {
        nyaterm_core::runtime::AppRuntime::installed_config_dir(
            home.ok_or("Cannot resolve home directory")?,
            flavor,
        )
    };
    Ok(config.join("mcp/discovery.json"))
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
    #[test]
    fn discovery_is_flavor_local_and_portable_markers_take_priority() {
        use nyaterm_core::app_identity::AppFlavor;
        let root = nyaterm_core::test_support::TestTempDir::new("nyaterm-mcp-path");
        std::fs::create_dir_all(root.path()).unwrap();
        let executable = root.join("nyaterm-mcp");
        let home = root.join("home");
        for flavor in [AppFlavor::Stable, AppFlavor::Preview] {
            assert_eq!(
                super::discovery_path_from(&executable, Some(&home), flavor).unwrap(),
                nyaterm_core::runtime::AppRuntime::installed_config_dir(&home, flavor)
                    .join("mcp/discovery.json")
            );
        }
        for marker in ["nyaterm-portable", "portable.flag"] {
            std::fs::write(root.join(marker), b"").unwrap();
            for flavor in [AppFlavor::Stable, AppFlavor::Preview] {
                assert_eq!(
                    super::discovery_path_from(&executable, None, flavor).unwrap(),
                    root.join("data/config/mcp/discovery.json")
                );
            }
            std::fs::remove_file(root.join(marker)).unwrap();
        }
    }

    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use nyaterm_mcp_protocol::{RpcRequest, RpcResponse};
    use serde_json::{Value, json};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::{TcpListener, TcpStream};
    use tokio_util::sync::CancellationToken;

    use super::{BridgeClient, BridgeEndpoint};

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
    #[tokio::test]
    async fn lost_response_resumes_the_same_owner_without_automatic_side_effect_replay() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let token = "a".repeat(64);
            for index in 0..2 {
                let (stream, _) = listener.accept().await.unwrap();
                let (reader, mut writer) = stream.into_split();
                let mut lines = BufReader::new(reader).lines();
                let auth = request(&mut lines).await;
                assert_eq!(auth.method, "auth");
                respond(
                    &mut writer,
                    auth.id,
                    json!({"authenticated":true,"features":["client_resume","mcp_lifecycle"]}),
                )
                .await;
                let attach = request(&mut lines).await;
                assert_eq!(attach.method, "client.attach");
                if index == 0 {
                    assert!(attach.params["resumeToken"].is_null());
                } else {
                    assert_eq!(attach.params["resumeToken"], token);
                }
                respond(
                    &mut writer,
                    attach.id,
                    json!({"ownerId":"owner","resumeToken":token}),
                )
                .await;
                let call = request(&mut lines).await;
                assert_eq!(call.method, "capability.execute");
                assert_eq!(call.params["arguments"]["idempotencyKey"], "key");
                if index == 1 {
                    respond(&mut writer, call.id, json!({"sessionId":"created-once"})).await;
                }
                // The first submitted operation survives a lost response at the Host.
            }
        });
        let client = BridgeClient::new(BridgeEndpoint::for_test(port));
        let args = json!({"connectionId":"saved","idempotencyKey":"key"});
        assert!(
            client
                .call("session_open", args.clone(), CancellationToken::new())
                .await
                .is_err()
        );
        assert!(client.supports_lifecycle());
        assert_eq!(
            client
                .call("session_open", args, CancellationToken::new())
                .await
                .unwrap()["sessionId"],
            "created-once"
        );
        server.await.unwrap();
    }
}

#[cfg(test)]
mod bridge_security_tests {
    use std::sync::Arc;
    use std::time::Duration;

    use nyaterm_mcp_protocol::{MAX_RPC_LINE_BYTES, RpcRequest, RpcResponse};
    use serde_json::{Value, json};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::{Notify, oneshot};
    use tokio_util::sync::CancellationToken;

    use super::{BridgeClient, BridgeEndpoint, read_rpc_line};

    async fn next_request(
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
    ) -> (
        tokio::io::Lines<BufReader<tokio::net::tcp::OwnedReadHalf>>,
        tokio::net::tcp::OwnedWriteHalf,
    ) {
        let (reader, mut writer) = stream.into_split();
        let mut lines = BufReader::new(reader).lines();
        let auth = next_request(&mut lines).await;
        assert_eq!(auth.method, "auth");
        respond(&mut writer, auth.id, json!({ "authenticated": true })).await;
        (lines, writer)
    }

    #[tokio::test]
    async fn bounded_rpc_reader_rejects_unterminated_and_oversized_lines() {
        let (mut writer, reader) = tokio::io::duplex(32);
        writer.write_all(b"{}").await.unwrap();
        writer.shutdown().await.unwrap();
        let error = read_rpc_line(&mut BufReader::new(reader))
            .await
            .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::UnexpectedEof);

        let (mut writer, reader) = tokio::io::duplex(64 * 1024);
        let writing = tokio::spawn(async move {
            for _ in 0..33 {
                writer.write_all(&vec![b'x'; 64 * 1024]).await.unwrap();
            }
        });
        let error = read_rpc_line(&mut BufReader::new(reader))
            .await
            .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        writing.abort();
    }

    #[tokio::test]
    async fn fake_host_timeout_invalidates_the_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (mut lines, _writer) = authenticate(stream).await;
            let request = next_request(&mut lines).await;
            assert_eq!(request.method, "capability.execute");
            tokio::time::sleep(Duration::from_secs(1)).await;
        });

        let endpoint = BridgeEndpoint::for_test_with_timeout(port, Duration::from_millis(25));
        let client = BridgeClient::new(endpoint);
        let error = client
            .call("get_environment", json!({}), CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(error.code, "bridge_timeout");
        server.abort();
    }

    #[tokio::test]
    async fn cancellation_uses_a_separate_authenticated_fake_host_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let call_received = Arc::new(Notify::new());
        let server_call_received = call_received.clone();
        let (cancel_seen_tx, cancel_seen_rx) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (mut call_lines, _call_writer) = authenticate(stream).await;
            let call = next_request(&mut call_lines).await;
            assert_eq!(call.method, "capability.execute");
            let request_id = call.params["requestId"].as_str().unwrap().to_string();
            server_call_received.notify_one();

            let (cancel_stream, _) = listener.accept().await.unwrap();
            let (mut cancel_lines, mut cancel_writer) = authenticate(cancel_stream).await;
            let cancel = next_request(&mut cancel_lines).await;
            assert_eq!(cancel.method, "request.cancel");
            assert_eq!(cancel.params["requestId"], request_id);
            respond(&mut cancel_writer, cancel.id, json!({ "cancelled": true })).await;
            let _ = cancel_seen_tx.send(());
        });

        let client = BridgeClient::new(BridgeEndpoint::for_test(port));
        let cancellation = CancellationToken::new();
        let task_token = cancellation.clone();
        let call_task =
            tokio::spawn(
                async move { client.call("get_environment", json!({}), task_token).await },
            );
        call_received.notified().await;
        cancellation.cancel();
        let error = call_task.await.unwrap().unwrap_err();
        assert_eq!(error.code, "cancelled");
        tokio::time::timeout(Duration::from_secs(2), cancel_seen_rx)
            .await
            .expect("cancel request timeout")
            .expect("cancel request observed");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn malformed_and_oversized_fake_host_responses_fail_closed() {
        for oversized in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let server = tokio::spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();
                let (mut lines, mut writer) = authenticate(stream).await;
                let _ = next_request(&mut lines).await;
                if oversized {
                    writer
                        .write_all(&vec![b'x'; MAX_RPC_LINE_BYTES + 1])
                        .await
                        .unwrap();
                } else {
                    writer.write_all(b"not-json\n").await.unwrap();
                }
            });
            let client = BridgeClient::new(BridgeEndpoint::for_test(port));
            let error = client
                .call("get_environment", json!({}), CancellationToken::new())
                .await
                .unwrap_err();
            assert_eq!(error.code, "bridge_disconnected");
            server.await.unwrap();
        }
    }
}
