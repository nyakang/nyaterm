//! Authenticated local HTTP ingress. The bridge identity belongs to an MCP session.
mod batch;
#[cfg(test)]
mod tests;
use crate::{bridge::BridgeEndpoint, handler::NyaTermMcp};
use bytes::Bytes;
use http::{HeaderValue, Method, Request, Response, StatusCode};
use http_body_util::{BodyExt, Full, Limited, combinators::UnsyncBoxBody};
use hyper::{body::Incoming, service::service_fn};
use hyper_util::rt::TokioIo;
use nyaterm_core::{
    SecretString,
    ai::mcp_http::{normalize_origin, validate_http_settings},
};
use nyaterm_mcp_protocol::http_control::{
    CONTROL_VERSION, HttpBootstrap, HttpControlEvent, MAX_CONTROL_BYTES,
};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, ListToolsResult, PaginatedRequestParams,
    ProtocolVersion, ServerInfo, Tool,
};
use rmcp::service::{NotificationContext, RequestContext};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService,
    session::{SessionManager, local::LocalSessionManager},
};
use rmcp::{ErrorData, RoleServer, ServerHandler};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::HashMap, convert::Infallible, sync::Arc, time::Duration};
use tokio::{
    io::AsyncWriteExt,
    net::TcpListener,
    sync::{Mutex, Semaphore},
};
use tokio_util::sync::CancellationToken;

const MAX_BODY: usize = 4 * 1024 * 1024;
type Body = UnsyncBoxBody<Bytes, Infallible>;
const VERSIONS: &[ProtocolVersion] = &[
    ProtocolVersion::V_2025_03_26,
    ProtocolVersion::V_2025_06_18,
    ProtocolVersion::V_2025_11_25,
];

struct HttpHandler(NyaTermMcp);
impl ServerHandler for HttpHandler {
    fn supported_protocol_versions(&self) -> std::borrow::Cow<'static, [ProtocolVersion]> {
        std::borrow::Cow::Borrowed(VERSIONS)
    }
    fn get_info(&self) -> ServerInfo {
        self.0.get_info()
    }
    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.0.get_tool(name)
    }
    async fn list_tools(
        &self,
        p: Option<PaginatedRequestParams>,
        cx: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        self.0.list_tools(p, cx).await
    }
    async fn call_tool(
        &self,
        p: CallToolRequestParams,
        cx: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        self.0.call_tool(p, cx).await
    }
    async fn on_initialized(&self, cx: NotificationContext<RoleServer>) {
        self.0.on_initialized(cx).await;
    }
}

struct HttpServer {
    service: StreamableHttpService<HttpHandler, LocalSessionManager>,
    sessions: Arc<LocalSessionManager>,
    versions: Mutex<HashMap<String, String>>,
    token_hash: [u8; 32],
    port: u16,
    origins: Vec<String>,
}
impl HttpServer {
    fn new(
        endpoint: BridgeEndpoint,
        port: u16,
        token: &str,
        origins: Vec<String>,
        ct: CancellationToken,
    ) -> Self {
        let mut manager = LocalSessionManager::default();
        manager.session_config.keep_alive = Some(Duration::from_secs(600));
        manager.session_config.init_timeout = Some(Duration::from_secs(10));
        let sessions = Arc::new(manager);
        let config = StreamableHttpServerConfig::default()
            .with_allowed_hosts([format!("127.0.0.1:{port}")])
            .with_cancellation_token(ct)
            .with_max_request_body_bytes(MAX_BODY);
        // Never clone a handler: BridgeClient::clone shares the owner's recovery credential.
        let service = StreamableHttpService::new(
            move || Ok(HttpHandler(NyaTermMcp::new(endpoint.clone()))),
            sessions.clone(),
            config,
        );
        Self {
            service,
            sessions,
            versions: Mutex::new(HashMap::new()),
            token_hash: Sha256::digest(token.as_bytes()).into(),
            port,
            origins,
        }
    }
    fn authorize(&self, request: &Request<Incoming>) -> Result<Option<String>, StatusCode> {
        if request.uri().path() != "/mcp" || request.uri().query().is_some() {
            return Err(StatusCode::NOT_FOUND);
        }
        if request.headers().get_all("host").iter().count() != 1
            || request.headers().get("host").and_then(|v| v.to_str().ok())
                != Some(format!("127.0.0.1:{}", self.port).as_str())
        {
            return Err(StatusCode::FORBIDDEN);
        }
        let origins: Vec<_> = request.headers().get_all("origin").iter().collect();
        let origin = match origins.as_slice() {
            [] => None,
            [raw] => {
                let normalized = raw
                    .to_str()
                    .ok()
                    .and_then(|value| normalize_origin(value).ok())
                    .ok_or(StatusCode::FORBIDDEN)?;
                if !self.origins.contains(&normalized) {
                    return Err(StatusCode::FORBIDDEN);
                }
                Some(normalized)
            }
            _ => return Err(StatusCode::FORBIDDEN),
        };
        if request.method() == Method::OPTIONS {
            if origin.is_none() {
                return Err(StatusCode::FORBIDDEN);
            }
            let method = request
                .headers()
                .get("access-control-request-method")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            if !["POST", "GET", "DELETE"].contains(&method) {
                return Err(StatusCode::FORBIDDEN);
            }
            if let Some(headers) = request.headers().get("access-control-request-headers") {
                for header in headers
                    .to_str()
                    .map_err(|_| StatusCode::FORBIDDEN)?
                    .split(',')
                {
                    if ![
                        "authorization",
                        "content-type",
                        "mcp-session-id",
                        "mcp-protocol-version",
                        "last-event-id",
                        "accept",
                    ]
                    .contains(&header.trim().to_ascii_lowercase().as_str())
                    {
                        return Err(StatusCode::FORBIDDEN);
                    }
                }
            }
            return Ok(origin);
        }
        if request.headers().get_all("authorization").iter().count() != 1 {
            return Err(StatusCode::UNAUTHORIZED);
        }
        let token = request
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or(StatusCode::UNAUTHORIZED)?;
        let supplied: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        if supplied
            .iter()
            .zip(self.token_hash)
            .fold(0u8, |difference, (a, b)| difference | (a ^ b))
            != 0
        {
            return Err(StatusCode::UNAUTHORIZED);
        }
        Ok(origin)
    }
    async fn handle(&self, request: Request<Incoming>) -> Result<Response<Body>, Infallible> {
        let origin = match self.authorize(&request) {
            Ok(origin) => origin,
            Err(code) => {
                let mut result = response(code);
                if code == StatusCode::UNAUTHORIZED {
                    result
                        .headers_mut()
                        .insert("www-authenticate", HeaderValue::from_static("Bearer"));
                    if let Some(origin) = request
                        .headers()
                        .get("origin")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|v| normalize_origin(v).ok())
                        .filter(|v| self.origins.contains(v))
                        && let Ok(value) = HeaderValue::from_str(&origin)
                    {
                        result
                            .headers_mut()
                            .insert("access-control-allow-origin", value);
                    }
                    result
                        .headers_mut()
                        .insert("vary", HeaderValue::from_static("Origin"));
                    result
                        .headers_mut()
                        .insert("cache-control", HeaderValue::from_static("no-store"));
                }
                return Ok(result);
            }
        };
        let mut result = if request.method() == Method::OPTIONS {
            let mut response = response(StatusCode::NO_CONTENT);
            let headers = response.headers_mut();
            headers.insert(
                "access-control-allow-methods",
                HeaderValue::from_static("POST, GET, DELETE, OPTIONS"),
            );
            headers.insert("access-control-allow-headers",HeaderValue::from_static("Authorization, Content-Type, Mcp-Session-Id, MCP-Protocol-Version, Last-Event-ID, Accept"));
            if request
                .headers()
                .get("access-control-request-private-network")
                .is_some_and(|v| v == "true")
            {
                headers.insert(
                    "access-control-allow-private-network",
                    HeaderValue::from_static("true"),
                );
            }
            response
        } else {
            self.dispatch(request).await
        };
        result
            .headers_mut()
            .insert("cache-control", HeaderValue::from_static("no-store"));
        result.headers_mut().insert(
            "vary",
            HeaderValue::from_static(
                "Origin, Access-Control-Request-Method, Access-Control-Request-Headers",
            ),
        );
        if let Some(origin) = origin {
            if let Ok(value) = HeaderValue::from_str(&origin) {
                result
                    .headers_mut()
                    .insert("access-control-allow-origin", value);
            }
            result.headers_mut().insert(
                "access-control-expose-headers",
                HeaderValue::from_static("Mcp-Session-Id, MCP-Protocol-Version"),
            );
        }
        Ok(result)
    }
    async fn dispatch(&self, request: Request<Incoming>) -> Response<Body> {
        if request.headers().get_all("mcp-session-id").iter().count() > 1
            || request
                .headers()
                .get_all("mcp-protocol-version")
                .iter()
                .count()
                > 1
        {
            return response(StatusCode::BAD_REQUEST);
        }
        let session = request
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let version = if let Some(id) = &session {
            let mut versions = self.versions.lock().await;
            if !self
                .sessions
                .has_session(&id.clone().into())
                .await
                .unwrap_or(false)
            {
                versions.remove(id);
                return response(StatusCode::NOT_FOUND);
            }
            match versions.get(id) {
                Some(version) => Some(version.clone()),
                None => return response(StatusCode::NOT_FOUND),
            }
        } else {
            None
        };
        if let Some(header) = request.headers().get("mcp-protocol-version")
            && (!VERSIONS
                .iter()
                .any(|v| header.as_bytes() == v.as_str().as_bytes())
                || version
                    .as_ref()
                    .is_some_and(|v| header.as_bytes() != v.as_bytes()))
        {
            return response(StatusCode::BAD_REQUEST);
        }
        if request.method() != Method::POST {
            if session.is_none() && matches!(*request.method(), Method::GET | Method::DELETE) {
                return response(StatusCode::BAD_REQUEST);
            }
            let deleting = request.method() == Method::DELETE;
            let result = self.service.handle(request).await;
            if deleting
                && result.status().is_success()
                && let Some(id) = session
            {
                self.versions.lock().await.remove(&id);
            }
            return result.map(BodyExt::boxed_unsync);
        }
        let media = |name: &str, expected: &str| {
            request
                .headers()
                .get_all(name)
                .iter()
                .filter_map(|v| v.to_str().ok())
                .flat_map(|value| value.split(','))
                .any(|value| {
                    value
                        .split(';')
                        .next()
                        .is_some_and(|value| value.trim() == expected)
                })
        };
        if !media("content-type", "application/json") {
            return response(StatusCode::UNSUPPORTED_MEDIA_TYPE);
        }
        if !media("accept", "application/json") || !media("accept", "text/event-stream") {
            return response(StatusCode::NOT_ACCEPTABLE);
        }
        let (parts, body) = request.into_parts();
        let mut bytes = match tokio::time::timeout(
            Duration::from_secs(15),
            Limited::new(body, MAX_BODY).collect(),
        )
        .await
        {
            Ok(Ok(body)) => body.to_bytes(),
            Ok(Err(_)) => return response(StatusCode::PAYLOAD_TOO_LARGE),
            Err(_) => return response(StatusCode::REQUEST_TIMEOUT),
        };
        let mut message: Value = match serde_json::from_slice(&bytes) {
            Ok(value) => value,
            Err(_) => return response(StatusCode::BAD_REQUEST),
        };
        if let Some(items) = message.as_array() {
            if session.is_none()
                && items.len() == 1
                && items[0].get("method").and_then(Value::as_str) == Some("initialize")
                && items[0]
                    .pointer("/params/protocolVersion")
                    .and_then(Value::as_str)
                    == Some("2025-03-26")
            {
                message = items[0].clone();
                bytes = Bytes::from(serde_json::to_vec(&message).expect("JSON value"));
            } else {
                return self.batch(parts, message, version).await;
            }
        }
        if session.is_none() {
            if message.get("method").and_then(Value::as_str) != Some("initialize") {
                return response(StatusCode::BAD_REQUEST);
            }
            let requested = message
                .pointer("/params/protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or("");
            let negotiated = if VERSIONS.iter().any(|v| v.as_str() == requested) {
                requested
            } else {
                "2025-11-25"
            }
            .to_owned();
            if let Some(params) = message.get_mut("params").and_then(Value::as_object_mut) {
                params.insert("protocolVersion".into(), Value::String(negotiated.clone()));
                bytes = Bytes::from(serde_json::to_vec(&message).expect("JSON value"));
            }
            // Serialize admission, including failed/abandoned initializations, to enforce the cap.
            let mut versions = self.versions.lock().await;
            let ids: Vec<_> = versions.keys().cloned().collect();
            for id in ids {
                if !self
                    .sessions
                    .has_session(&id.clone().into())
                    .await
                    .unwrap_or(false)
                {
                    versions.remove(&id);
                }
            }
            if versions.len() >= 64 {
                return response(StatusCode::TOO_MANY_REQUESTS);
            }
            let result = self
                .service
                .handle(Request::from_parts(parts, Full::new(bytes)))
                .await;
            if let Some(id) = result
                .headers()
                .get("mcp-session-id")
                .and_then(|v| v.to_str().ok())
            {
                versions.insert(id.to_owned(), negotiated.to_owned());
            }
            return result.map(BodyExt::boxed_unsync);
        }
        self.service
            .handle(Request::from_parts(parts, Full::new(bytes)))
            .await
            .map(BodyExt::boxed_unsync)
    }
}
fn response(code: StatusCode) -> Response<Body> {
    Response::builder()
        .status(code)
        .body(Full::new(Bytes::new()).boxed_unsync())
        .expect("valid status")
}

async fn serve(
    listener: TcpListener,
    server: Arc<HttpServer>,
    ct: CancellationToken,
) -> std::io::Result<()> {
    let connections = Arc::new(Semaphore::new(128));
    let mut tasks = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            _=ct.cancelled()=>break,
            Some(_)=tasks.join_next()=>{},
            incoming=listener.accept()=>{
                let (stream,_)=incoming?;
                let Ok(permit)=connections.clone().try_acquire_owned() else {continue;};
                let server=server.clone(); let ct=ct.clone();
                tasks.spawn(async move {
                    let _permit=permit;
                    let service=service_fn(move |request| {let server=server.clone(); async move {server.handle(request).await}});
                    tokio::select! { _=ct.cancelled()=>{}, _=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(stream),service)=>{} }
                });
            }
        }
    }
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    Ok(())
}

pub(crate) async fn run_managed() -> Result<(), Box<dyn std::error::Error>> {
    let result = managed().await;
    if let Err(error) = &result {
        let code = if error
            .downcast_ref::<std::io::Error>()
            .is_some_and(|error| error.kind() == std::io::ErrorKind::AddrInUse)
        {
            "port_in_use"
        } else {
            "http_start_or_runtime_failed"
        };
        let event = HttpControlEvent::Failed {
            version: CONTROL_VERSION,
            code: code.into(),
        };
        let mut bytes = serde_json::to_vec(&event)?;
        bytes.push(b'\n');
        let _ = tokio::io::stdout().write_all(&bytes).await;
    }
    result.map_err(|_| "Managed HTTP MCP service failed".into())
}
async fn managed() -> Result<(), Box<dyn std::error::Error>> {
    // Tokio's stdin uses an uncancellable blocking-pool read, which can keep the
    // runtime alive after a failed listener. A detached OS reader dies with the
    // sidecar; EOF or any control byte requests shutdown without blocking Tokio.
    let ct = CancellationToken::new();
    let reader_stop = ct.clone();
    let (bootstrap_tx, bootstrap_rx) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name("mcp-http-control".into())
        .spawn(move || {
            use std::io::{BufRead, Read};
            let mut input = std::io::BufReader::new(std::io::stdin());
            let mut line = Vec::new();
            let result = (&mut input)
                .take((MAX_CONTROL_BYTES + 1) as u64)
                .read_until(b'\n', &mut line);
            let _ = bootstrap_tx.send(result.map(|_| line));
            let mut byte = [0];
            let _ = input.read(&mut byte);
            reader_stop.cancel();
        })?;
    let line = tokio::time::timeout(Duration::from_secs(10), bootstrap_rx).await???;
    if line.len() > MAX_CONTROL_BYTES || line.last() != Some(&b'\n') {
        return Err("Invalid bootstrap".into());
    }
    let config: HttpBootstrap = serde_json::from_slice(&line)?;
    if config.version != CONTROL_VERSION
        || config.bridge_port == 0
        || config.bridge_token.is_empty()
        || config.bridge_generation.is_empty()
    {
        return Err("Invalid bootstrap".into());
    }
    let origins = validate_http_settings(config.port, &config.allowed_origins)?;
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, config.port)).await?;
    let token = SecretString::new(hex::encode(rand::random::<[u8; 32]>()));
    if ct.is_cancelled() {
        return Err("Parent disconnected".into());
    }
    let server = Arc::new(HttpServer::new(
        BridgeEndpoint::managed(
            config.bridge_port,
            config.bridge_token,
            config.bridge_generation,
        ),
        config.port,
        token.expose_secret(),
        origins,
        ct.clone(),
    ));
    let ready = HttpControlEvent::Ready {
        version: CONTROL_VERSION,
        port: config.port,
        token: token.expose_secret().into(),
    };
    let mut bytes = serde_json::to_vec(&ready)?;
    bytes.push(b'\n');
    tokio::io::stdout().write_all(&bytes).await?;
    tokio::io::stdout().flush().await?;
    serve(listener, server, ct).await?;
    Ok(())
}
