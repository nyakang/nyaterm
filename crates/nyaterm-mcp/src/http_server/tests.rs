use super::{HttpServer, serve};
use crate::bridge::BridgeEndpoint;
use serde_json::json;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn browser_authentication_sessions_and_route_isolation() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let stop = CancellationToken::new();
    let server = Arc::new(HttpServer::new(
        BridgeEndpoint::for_test(1),
        port,
        "fixture-token",
        vec!["https://client.example".into()],
        stop.clone(),
    ));
    let task = tokio::spawn(serve(listener, server, stop.clone()));
    let url = format!("http://127.0.0.1:{port}/mcp");
    let client = reqwest::Client::new();
    assert_eq!(client.get(&url).send().await.unwrap().status(), 401);
    assert_eq!(
        client
            .get(format!("{url}/other"))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(
        client
            .get(&url)
            .bearer_auth("fixture-token")
            .header("origin", "https://evil.example")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let preflight = client
        .request(reqwest::Method::OPTIONS, &url)
        .header("origin", "https://client.example")
        .header("access-control-request-method", "POST")
        .header(
            "access-control-request-headers",
            "authorization,content-type,mcp-session-id",
        )
        .send()
        .await
        .unwrap();
    assert_eq!(preflight.status(), 204);
    assert_eq!(
        preflight.headers()["access-control-allow-origin"],
        "https://client.example"
    );
    let mut session_ids = Vec::new();
    for version in ["2025-03-26", "2025-06-18", "2025-11-25"] {
        let result=client.post(&url).bearer_auth("fixture-token").header("accept","application/json, text/event-stream").header("origin","https://client.example").json(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":version,"capabilities":{},"clientInfo":{"name":"test","version":"1"}}})).send().await.unwrap();
        assert!(
            result.status().is_success(),
            "{version}: {}",
            result.status()
        );
        let id = result.headers()["mcp-session-id"]
            .to_str()
            .unwrap()
            .to_owned();
        assert_eq!(
            result.headers()["access-control-expose-headers"],
            "Mcp-Session-Id, MCP-Protocol-Version"
        );
        let text = result.text().await.unwrap();
        assert!(text.contains(version), "{text}");
        let initialized = client
            .post(&url)
            .bearer_auth("fixture-token")
            .header("accept", "application/json, text/event-stream")
            .header("mcp-session-id", &id)
            .header("mcp-protocol-version", version)
            .json(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .send()
            .await
            .unwrap();
        assert_eq!(initialized.status(), 202);
        assert_eq!(
            client
                .get(&url)
                .header("mcp-session-id", &id)
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
        session_ids.push(id);
    }
    assert_ne!(session_ids[0], session_ids[1]);
    let id = &session_ids[0];
    let batch=client.post(&url).bearer_auth("fixture-token").header("accept","application/json, text/event-stream").header("mcp-session-id",id).json(&json!([{"jsonrpc":"2.0","id":2,"method":"ping"},{"jsonrpc":"2.0","id":3,"method":"ping"}])).send().await.unwrap();
    assert_eq!(batch.status(), 200);
    let text = batch.text().await.unwrap();
    assert!(
        text.contains("\"id\":2") && text.contains("\"id\":3"),
        "{text}"
    );
    assert!(
        client
            .delete(&url)
            .bearer_auth("fixture-token")
            .header("mcp-session-id", id)
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    assert_eq!(
        client
            .get(&url)
            .bearer_auth("fixture-token")
            .header("mcp-session-id", id)
            .header("accept", "text/event-stream")
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    stop.cancel();
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn tools_calls_use_distinct_bridge_owners_and_origin_denial_is_fail_closed() {
    use nyaterm_mcp_protocol::{RpcRequest, RpcResponse};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let bridge = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let bridge_port = bridge.local_addr().unwrap().port();
    let connections = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = connections.clone();
    let bridge_task = tokio::spawn(async move {
        loop {
            let (socket, _) = bridge.accept().await.unwrap();
            let owner = count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            tokio::spawn(async move {
                let (read, mut write) = socket.into_split();
                let mut lines = BufReader::new(read).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let request: RpcRequest = serde_json::from_str(&line).unwrap();
                    let value = match request.method.as_str() {
                        "auth" => {
                            json!({"authenticated":true,"features":["client_resume","terminal_lifecycle","connection_targets"]})
                        }
                        "client.attach" => {
                            json!({"connectionId":format!("owner-{owner}"),"resumeToken":format!("{owner:064x}")})
                        }
                        "client.identify" => json!({"identified":true}),
                        "capability.execute" => json!({"connections":[]}),
                        _ => json!({}),
                    };
                    let response = RpcResponse {
                        id: request.id,
                        result: Some(value),
                        error: None,
                    };
                    let mut bytes = serde_json::to_vec(&response).unwrap();
                    bytes.push(b'\n');
                    if write.write_all(&bytes).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let stop = CancellationToken::new();
    let server = Arc::new(HttpServer::new(
        BridgeEndpoint::for_test(bridge_port),
        port,
        "fixture",
        vec![],
        stop.clone(),
    ));
    let task = tokio::spawn(serve(listener, server, stop.clone()));
    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{port}/mcp");
    for _ in 0..2 {
        let init=client.post(&url).bearer_auth("fixture").header("accept","application/json, text/event-stream").json(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1"}}})).send().await.unwrap();
        let id = init.headers()["mcp-session-id"]
            .to_str()
            .unwrap()
            .to_owned();
        let _ = init.text().await.unwrap();
        let request = |value| {
            client
                .post(&url)
                .bearer_auth("fixture")
                .header("accept", "application/json, text/event-stream")
                .header("mcp-session-id", &id)
                .header("mcp-protocol-version", "2025-11-25")
                .json(&value)
        };
        assert_eq!(
            request(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
                .send()
                .await
                .unwrap()
                .status(),
            202
        );
        let list = request(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(list.contains("connection_list"), "{list}");
        let call=request(json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"connection_list","arguments":{}}})).send().await.unwrap().text().await.unwrap();
        assert!(
            call.contains("structuredContent") && call.contains("\"connections\":[]"),
            "{call}"
        );
        assert_eq!(
            request(json!({"jsonrpc":"2.0","id":4,"method":"ping"}))
                .header("origin", "null")
                .send()
                .await
                .unwrap()
                .status(),
            403
        );
    }
    assert_eq!(
        connections.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "HTTP clients must not share a bridge identity"
    );
    stop.cancel();
    task.await.unwrap().unwrap();
    bridge_task.abort();
}

#[tokio::test]
async fn http_disconnect_does_not_cancel_or_replay_but_explicit_cancellation_reaches_bridge() {
    use nyaterm_mcp_protocol::{RpcRequest, RpcResponse};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let bridge = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let bridge_port = bridge.local_addr().unwrap().port();
    let (events, mut received) = tokio::sync::mpsc::unbounded_channel();
    let bridge_task = tokio::spawn(async move {
        loop {
            let (socket, _) = bridge.accept().await.unwrap();
            let events = events.clone();
            tokio::spawn(async move {
                let (read, mut write) = socket.into_split();
                let mut lines = BufReader::new(read).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let request: RpcRequest = serde_json::from_str(&line).unwrap();
                    let result = match request.method.as_str() {
                        "auth" => json!({"authenticated":true,"features":["client_resume"]}),
                        "client.attach" => {
                            json!({"connectionId":"owner","resumeToken":"a".repeat(64)})
                        }
                        "client.identify" => json!({"identified":true}),
                        "capability.execute" => {
                            let _ = events.send("started");
                            continue;
                        }
                        "request.cancel" => {
                            let _ = events.send("cancelled");
                            json!({"cancelled":true})
                        }
                        _ => json!({}),
                    };
                    let mut bytes = serde_json::to_vec(&RpcResponse {
                        id: request.id,
                        result: Some(result),
                        error: None,
                    })
                    .unwrap();
                    bytes.push(b'\n');
                    if write.write_all(&bytes).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let stop = CancellationToken::new();
    let server = Arc::new(HttpServer::new(
        BridgeEndpoint::for_test(bridge_port),
        port,
        "fixture",
        vec![],
        stop.clone(),
    ));
    let task = tokio::spawn(serve(listener, server, stop.clone()));
    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{port}/mcp");
    let init=client.post(&url).bearer_auth("fixture").header("accept","application/json, text/event-stream").json(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1"}}})).send().await.unwrap();
    let id = init.headers()["mcp-session-id"]
        .to_str()
        .unwrap()
        .to_owned();
    let _ = init.text().await.unwrap();
    let request = |body| {
        client
            .post(&url)
            .bearer_auth("fixture")
            .header("accept", "application/json, text/event-stream")
            .header("mcp-session-id", &id)
            .json(&body)
    };
    assert_eq!(
        request(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .send()
            .await
            .unwrap()
            .status(),
        202
    );
    let stream=request(json!({"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"connection_list","arguments":{}}})).send().await.unwrap();
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(2), received.recv())
            .await
            .unwrap(),
        Some("started")
    );
    drop(stream);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), received.recv())
            .await
            .is_err()
    );
    assert_eq!(request(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":9,"reason":"fixture"}})).send().await.unwrap().status(),202);
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(2), received.recv())
            .await
            .unwrap(),
        Some("cancelled")
    );
    stop.cancel();
    task.await.unwrap().unwrap();
    bridge_task.abort();
}

#[tokio::test]
async fn http_admission_bounds_sessions_and_payloads_before_dispatch() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let stop = CancellationToken::new();
    let server = Arc::new(HttpServer::new(
        BridgeEndpoint::for_test(1),
        port,
        "fixture",
        vec![],
        stop.clone(),
    ));
    let task = tokio::spawn(serve(listener, server, stop.clone()));
    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{port}/mcp");
    let init = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1"}}});
    let post = || {
        client
            .post(&url)
            .bearer_auth("fixture")
            .header("accept", "application/json, text/event-stream")
    };
    let mut first = None;
    for _ in 0..64 {
        let response = post().json(&init).send().await.unwrap();
        assert!(response.status().is_success());
        if first.is_none() {
            first = Some(
                response.headers()["mcp-session-id"]
                    .to_str()
                    .unwrap()
                    .to_owned(),
            );
        }
        let _ = response.text().await.unwrap();
    }
    assert_eq!(post().json(&init).send().await.unwrap().status(), 429);
    assert!(
        client
            .delete(&url)
            .bearer_auth("fixture")
            .header("mcp-session-id", first.unwrap())
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    assert!(
        post()
            .json(&init)
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    assert_eq!(
        post()
            .header("content-type", "application/json")
            .body(vec![b' '; super::MAX_BODY + 1])
            .send()
            .await
            .unwrap()
            .status(),
        413
    );
    assert_eq!(
        post()
            .header("content-type", "text/plain")
            .body("{}")
            .send()
            .await
            .unwrap()
            .status(),
        415
    );
    assert_eq!(
        client
            .post(&url)
            .bearer_auth("fixture")
            .header("accept", "text/event-stream")
            .json(&init)
            .send()
            .await
            .unwrap()
            .status(),
        406
    );
    stop.cancel();
    task.await.unwrap().unwrap();
}
