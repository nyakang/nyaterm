use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nyaterm_mcp_protocol::{RpcError, RpcRequest, RpcResponse, TerminalStartArgs, tool};
use rmcp::ServiceExt;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::Notify;
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::{DEFAULT_WAIT_MS, MAX_WAIT_MS, StartWait, input_schema, result_schema};
use crate::bridge::{BridgeClient, BridgeEndpoint};
use crate::{NyaTermMcp, build_tools};

enum Reply {
    Value(Value),
    Error(&'static str),
    Delayed(Duration, Value),
}

struct Host {
    bridge: BridgeClient,
    requests: Arc<Mutex<Vec<RpcRequest>>>,
    changed: Arc<Notify>,
    task: Option<JoinHandle<()>>,
}

impl Host {
    async fn new(start: Value, replies: Vec<Reply>, targets: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let bridge = BridgeClient::new(BridgeEndpoint::for_test(
            listener.local_addr().unwrap().port(),
        ));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let changed = Arc::new(Notify::new());
        let replies = Arc::new(Mutex::new(VecDeque::from(replies)));
        let seen = requests.clone();
        let notification = changed.clone();
        let task = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let (stream, _) = accepted.unwrap();
                        let start = start.clone();
                        let replies = replies.clone();
                        let seen = seen.clone();
                        let notification = notification.clone();
                        connections.spawn(async move {
                            let (reader, mut writer) = stream.into_split();
                            let mut lines = BufReader::new(reader).lines();
                            while let Some(line) = lines.next_line().await.unwrap() {
                                let request: RpcRequest = serde_json::from_str(&line).unwrap();
                                let reply = match request.method.as_str() {
                                    "auth" => Reply::Value(json!({"authenticated":true,
                                        "features": if targets {vec!["mcp_lifecycle", "connection_targets"]}
                                            else {vec!["mcp_lifecycle"]}})),
                                    "client.identify" => Reply::Value(json!({"identified":true})),
                                    "request.cancel" => Reply::Value(json!({"cancelled":true})),
                                    "capability.execute" => match request.params["tool"].as_str().unwrap() {
                                        tool::TERMINAL_START => {
                                            assert!(request.params["arguments"].get("waitMs").is_none());
                                            Reply::Value(start.clone())
                                        }
                                        tool::TERMINAL_POLL => replies.lock().unwrap().pop_front()
                                            .expect("unexpected extra poll"),
                                        other => panic!("unexpected tool: {other}"),
                                    },
                                    other => panic!("unexpected method: {other}"),
                                };
                                let id = request.id;
                                seen.lock().unwrap().push(request);
                                notification.notify_one();
                                let result = match reply {
                                    Reply::Value(value) => Ok(value),
                                    Reply::Error(code) => Err(RpcError {code:code.into(), message:"mock rejection".into()}),
                                    Reply::Delayed(delay, value) => {
                                        tokio::time::sleep(delay).await;
                                        Ok(value)
                                    }
                                };
                                let response = match result {
                                    Ok(value) => RpcResponse {id, result:Some(value), error:None},
                                    Err(error) => RpcResponse {id, result:None, error:Some(error)},
                                };
                                let mut bytes = serde_json::to_vec(&response).unwrap();
                                bytes.push(b'\n');
                                // Deadline/cancellation may have already closed this socket.
                                if writer.write_all(&bytes).await.is_err() {
                                    break;
                                }
                            }
                        });
                    }
                    completed = connections.join_next(), if !connections.is_empty() => {
                        completed.unwrap().expect("mock connection panicked");
                    }
                }
            }
        });
        bridge.ensure_connected().await;
        Self {
            bridge,
            requests,
            changed,
            task: Some(task),
        }
    }

    fn count(&self, name: &str) -> usize {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.method == name || request.params["tool"] == name)
            .count()
    }

    async fn wait_for(&self, name: &str, count: usize) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let notified = self.changed.notified();
                if self.count(name) >= count {
                    return;
                }
                notified.await;
            }
        })
        .await
        .expect("mock request deadline");
    }

    async fn mcp_call(&self, arguments: Value) -> Value {
        let server = NyaTermMcp {
            bridge: self.bridge.clone(),
            tools: Arc::new(build_tools()),
        };
        let (server_io, client_io) = tokio::io::duplex(64 * 1024);
        let task = tokio::spawn(async move {
            server
                .serve(server_io)
                .await
                .unwrap()
                .waiting()
                .await
                .unwrap();
        });
        let (reader, mut writer) = tokio::io::split(client_io);
        let mut lines = BufReader::new(reader).lines();
        for message in [
            json!({"jsonrpc":"2.0", "id":1, "method":"initialize", "params":{
                "protocolVersion":"2025-11-25", "capabilities":{},
                "clientInfo":{"name":"start-wait-test", "version":"1.0.0"}}}),
            json!({"jsonrpc":"2.0", "method":"notifications/initialized"}),
            json!({"jsonrpc":"2.0", "id":2, "method":"tools/call", "params":{
                "name":tool::TERMINAL_START, "arguments":arguments}}),
        ] {
            let mut bytes = serde_json::to_vec(&message).unwrap();
            bytes.push(b'\n');
            writer.write_all(&bytes).await.unwrap();
            if message["id"] == 1 {
                lines.next_line().await.unwrap().unwrap();
            }
        }
        let response = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let line = lines.next_line().await.unwrap().unwrap();
                let value: Value = serde_json::from_str(&line).unwrap();
                if value["id"] == 2 {
                    break value;
                }
            }
        })
        .await
        .expect("MCP response deadline");
        drop(writer);
        drop(lines);
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        response
    }

    async fn finish(mut self) {
        tokio::task::yield_now().await;
        let task = self.task.take().unwrap();
        task.abort();
        let error = task.await.expect_err("mock must run until shutdown");
        assert!(error.is_cancelled(), "mock host panicked: {error}");
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

fn start(connection: bool, backend: &str, status: &str) -> Value {
    let mut value = json!({"jobId":"job-1", "backend":backend, "status":status});
    if connection {
        value["target"] = json!({"type":"connection", "connectionId":"c1"});
    } else {
        value["sessionId"] = json!("s1");
    }
    value
}

fn poll(start: &Value, status: &str, output: &str) -> Value {
    let transient = matches!(status, "starting" | "running" | "cancelling");
    let mut value = start.clone();
    value["status"] = json!(status);
    let fields = json!({"processState":if transient {"active"} else {"exit_confirmed"},
        "exitCode":if transient {None} else {Some(0)}, "startedAtMs":1,
        "endedAtMs":if transient {None} else {Some(2)}, "output":output,
        "offset":0, "nextOffset":output.len(), "baseOffset":0, "totalBytes":output.len(),
        "eof":true, "outputComplete":!transient, "sourceTruncated":false,
        "droppedBeforeOffset":false, "stopState":"not_requested"});
    value
        .as_object_mut()
        .unwrap()
        .extend(fields.as_object().unwrap().clone());
    if start.get("sessionId").is_some() {
        value.as_object_mut().unwrap().extend(
            json!({"terminalState":"ready",
            "sessionExists":true, "connected":true})
            .as_object()
            .unwrap()
            .clone(),
        );
    }
    value
}

#[test]
fn local_wait_validation_and_schemas_agree_without_changing_host_arguments() {
    for targets in [false, true] {
        let schema = input_schema(tool::TERMINAL_START, targets).unwrap();
        let validator = jsonschema::validator_for(&schema).unwrap();
        for wait in [
            Value::Null,
            json!(0),
            json!(1),
            json!(30000),
            json!(60000),
            json!(u64::MAX),
        ] {
            let mut arguments = json!({"command":"echo test", "waitMs":wait});
            assert!(validator.is_valid(&arguments));
            let policy = StartWait::take(tool::TERMINAL_START, &mut arguments).unwrap();
            assert_eq!(arguments, json!({"command":"echo test"}));
            assert!(policy.wait_ms(&start(false, "ssh_exec", "starting")) <= MAX_WAIT_MS);
        }
        for wait in [
            json!(-1),
            json!(0.5),
            json!("100"),
            json!(true),
            json!([]),
            json!({}),
        ] {
            let mut arguments = json!({"command":"echo test", "waitMs":wait});
            assert!(!validator.is_valid(&arguments));
            assert!(StartWait::take(tool::TERMINAL_START, &mut arguments).is_err());
        }
        let host_schema =
            nyaterm_mcp_protocol::tool_input_schema_for_features(tool::TERMINAL_START, targets)
                .unwrap();
        assert!(host_schema["properties"].get("waitMs").is_none());
    }
    let mut arguments = json!({"command":"echo test", "waitMs":MAX_WAIT_MS + 1});
    assert_eq!(
        StartWait::take(tool::TERMINAL_START, &mut arguments)
            .unwrap()
            .0,
        Some(MAX_WAIT_MS)
    );
    let canonical: TerminalStartArgs = serde_json::from_value(arguments).unwrap();
    assert!(
        serde_json::to_value(canonical)
            .unwrap()
            .get("waitMs")
            .is_none()
    );
    let mut other = json!({"jobId":"job-1", "waitMs":0});
    StartWait::take(tool::TERMINAL_POLL, &mut other).unwrap();
    assert!(nyaterm_mcp_protocol::validate_tool_arguments(tool::TERMINAL_POLL, &other).is_err());
}

#[test]
fn default_wait_uses_the_actual_backend() {
    let default = StartWait::default();
    assert_eq!(
        default.wait_ms(&start(true, "ssh_exec", "starting")),
        DEFAULT_WAIT_MS
    );
    assert_eq!(
        default.wait_ms(&start(false, "ssh_exec", "starting")),
        DEFAULT_WAIT_MS
    );
    assert_eq!(default.wait_ms(&start(false, "inline_pty", "starting")), 0);
    assert_eq!(
        StartWait(Some(0)).wait_ms(&start(true, "ssh_exec", "starting")),
        0
    );
    assert_eq!(
        StartWait(Some(100)).wait_ms(&start(false, "inline_pty", "starting")),
        100
    );
}

#[test]
fn advertised_output_schema_accepts_start_and_poll_for_negotiated_targets() {
    for targets in [false, true] {
        let schema = result_schema(tool::TERMINAL_START, targets).unwrap();
        assert_eq!(schema["type"], "object");
        let validator = jsonschema::validator_for(&schema).unwrap();
        for connection in [false, true] {
            let start = start(connection, "ssh_exec", "starting");
            for value in [
                start.clone(),
                poll(&start, "running", ""),
                poll(&start, "completed", "中文"),
            ] {
                assert_eq!(validator.is_valid(&value), !connection || targets);
            }
        }
        assert!(!validator.is_valid(&json!({"jobId":"job-1"})));
    }
}

#[tokio::test]
async fn default_wait_follows_host_overrides_in_both_directions() {
    let inline = start(false, "inline_pty", "starting");
    let host = Host::new(inline.clone(), vec![], true).await;
    let response = host
        .mcp_call(json!({"command":"echo test", "backend":"ssh_exec",
        "target":{"type":"connection", "connectionId":"c1"}}))
        .await;
    assert_eq!(response["result"]["structuredContent"], inline);
    assert_eq!(host.count(tool::TERMINAL_POLL), 0);
    host.finish().await;

    let exec = start(false, "ssh_exec", "starting");
    let completed = poll(&exec, "completed", "ok");
    let host = Host::new(exec, vec![Reply::Value(completed.clone())], false).await;
    let response = host
        .mcp_call(json!({"command":"echo test", "backend":"inline_pty", "sessionId":"s1"}))
        .await;
    assert_eq!(response["result"]["structuredContent"], completed);
    host.finish().await;
}

#[tokio::test]
async fn malformed_start_returns_protocol_error_without_polling() {
    let host = Host::new(
        json!({"jobId":"job-1", "backend":"ssh_exec", "status":"broken"}),
        vec![],
        true,
    )
    .await;
    let response = host.mcp_call(json!({"command":"echo test"})).await;
    assert_eq!(response["result"]["isError"], true);
    assert_eq!(
        response["result"]["structuredContent"]["code"],
        "bridge_protocol_error"
    );
    assert_eq!(host.count(tool::TERMINAL_POLL), 0);
    host.finish().await;
}

#[tokio::test]
async fn invalid_wait_is_rejected_before_command_submission() {
    let host = Host::new(start(true, "ssh_exec", "starting"), vec![], true).await;
    let response = host
        .mcp_call(json!({"command":"echo test", "waitMs":-1}))
        .await;
    assert_eq!(response["error"]["code"], -32602);
    assert_eq!(host.count(tool::TERMINAL_START), 0);
    host.finish().await;
}

#[tokio::test]
async fn changing_wait_does_not_change_forwarded_idempotency_arguments() {
    let initial = start(true, "ssh_exec", "starting");
    let host = Host::new(
        initial.clone(),
        vec![Reply::Value(poll(&initial, "completed", "ok"))],
        true,
    )
    .await;
    for wait in [0, 500] {
        host.mcp_call(
            json!({"command":"echo test", "idempotencyKey":"same-key", "waitMs":wait,
            "target":{"type":"connection", "connectionId":"c1"}}),
        )
        .await;
    }
    let forwarded: Vec<Value> = host
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter(|request| request.params["tool"] == tool::TERMINAL_START)
        .map(|request| request.params["arguments"].clone())
        .collect();
    assert_eq!(forwarded.len(), 2);
    assert_eq!(forwarded[0], forwarded[1]);
    host.finish().await;
}

#[tokio::test]
async fn explicit_inline_wait_reads_completed_paged_output_without_accumulating() {
    let initial = start(false, "inline_pty", "starting");
    let running = poll(&initial, "running", "prefix");
    let mut completed = poll(&initial, "completed", "prefix 中文");
    completed["eof"] = json!(false);
    completed["totalBytes"] = json!(100_000);
    completed["sourceTruncated"] = json!(true);
    let host = Host::new(
        initial,
        vec![Reply::Value(running), Reply::Value(completed.clone())],
        false,
    )
    .await;
    let response = host
        .mcp_call(json!({"command":"echo test", "waitMs":1000}))
        .await;
    assert_eq!(response["result"]["structuredContent"], completed);
    assert_eq!(host.count(tool::TERMINAL_POLL), 2);
    assert!(
        host.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.params["tool"] == tool::TERMINAL_POLL)
            .all(|request| request.params["arguments"] == json!({"jobId":"job-1"}))
    );
    host.finish().await;
}

#[tokio::test]
async fn terminal_start_status_still_fetches_output_once() {
    let initial = start(true, "ssh_exec", "completed");
    let completed = poll(&initial, "completed", "ok");
    let host = Host::new(initial.clone(), vec![Reply::Value(completed.clone())], true).await;
    assert_eq!(
        StartWait::default()
            .wait_for_result(&host.bridge, initial, CancellationToken::new())
            .await,
        completed
    );
    assert_eq!(host.count(tool::TERMINAL_POLL), 1);
    host.finish().await;
}

#[tokio::test]
async fn every_terminal_poll_status_finishes_waiting() {
    for status in [
        "completed",
        "failed",
        "timed_out",
        "cancelled",
        "disconnected",
        "unknown",
    ] {
        let initial = start(true, "ssh_exec", "starting");
        let terminal = poll(&initial, status, "output");
        let host = Host::new(initial.clone(), vec![Reply::Value(terminal.clone())], true).await;
        assert_eq!(
            StartWait(Some(500))
                .wait_for_result(&host.bridge, initial, CancellationToken::new())
                .await,
            terminal
        );
        assert_eq!(host.count(tool::TERMINAL_POLL), 1);
        host.finish().await;
    }
}

#[tokio::test]
async fn poll_error_or_malformed_response_preserves_last_valid_result() {
    for error in [
        Reply::Error("job_not_found"),
        Reply::Value(json!({"jobId":"job-1", "status":"running"})),
        Reply::Value(poll(
            &start(true, "ssh_exec", "starting"),
            "completed",
            "wrong job",
        )),
    ] {
        let initial = start(true, "ssh_exec", "starting");
        let running = poll(&initial, "running", "retained");
        let error = match error {
            Reply::Value(mut value) if value.get("output").is_some() => {
                value["jobId"] = json!("other-job");
                Reply::Value(value)
            }
            error => error,
        };
        let host = Host::new(
            initial.clone(),
            vec![Reply::Value(running.clone()), error],
            true,
        )
        .await;
        assert_eq!(
            StartWait(Some(1000))
                .wait_for_result(&host.bridge, initial, CancellationToken::new())
                .await,
            running
        );
        host.finish().await;
    }
}

#[tokio::test]
async fn first_poll_error_preserves_the_start_response() {
    let initial = start(true, "ssh_exec", "starting");
    let host = Host::new(initial.clone(), vec![Reply::Error("denied")], true).await;
    let response = host
        .mcp_call(json!({"command":"echo test", "waitMs":500}))
        .await;
    assert_eq!(response["result"]["structuredContent"], initial);
    assert_ne!(response["result"]["isError"], true);
    host.finish().await;
}

#[tokio::test]
async fn deadline_before_first_poll_returns_start_without_rpc() {
    let initial = start(true, "ssh_exec", "starting");
    let host = Host::new(initial.clone(), vec![], true).await;
    tokio::time::pause();
    let began = Instant::now();
    assert_eq!(
        StartWait(Some(30))
            .wait_for_result(&host.bridge, initial.clone(), CancellationToken::new())
            .await,
        initial
    );
    // Tokio's timer wheel rounds deadlines up to the next millisecond.
    assert!((Duration::from_millis(30)..=Duration::from_millis(31)).contains(&began.elapsed()));
    assert_eq!(host.count(tool::TERMINAL_POLL), 0);
    host.finish().await;
}

#[tokio::test]
async fn slow_poll_uses_remaining_budget_and_next_call_reconnects() {
    let initial = start(true, "ssh_exec", "starting");
    let completed = poll(&initial, "completed", "ok");
    let host = Host::new(
        initial.clone(),
        vec![
            Reply::Delayed(Duration::from_millis(1100), completed.clone()),
            Reply::Value(completed.clone()),
            Reply::Value(completed.clone()),
        ],
        true,
    )
    .await;
    let began = Instant::now();
    assert_eq!(
        StartWait(Some(350))
            .wait_for_result(&host.bridge, initial.clone(), CancellationToken::new())
            .await,
        initial
    );
    assert!(began.elapsed() < Duration::from_millis(900));
    assert_eq!(
        host.bridge
            .call(
                tool::TERMINAL_POLL,
                json!({"jobId":"job-1"}),
                CancellationToken::new()
            )
            .await
            .unwrap(),
        completed
    );
    assert_eq!(host.count("auth"), 2);
    // Let the abandoned socket produce its late response, then verify that
    // the live connection still reads its own response without another auth.
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(
        host.bridge
            .call(
                tool::TERMINAL_POLL,
                json!({"jobId":"job-1"}),
                CancellationToken::new()
            )
            .await
            .unwrap(),
        completed
    );
    assert_eq!(host.count("auth"), 2);
    assert_eq!(host.count(tool::TERMINAL_START), 0);
    host.finish().await;
}

#[tokio::test]
async fn deadline_includes_connection_lock_contention_without_closing_other_call() {
    let initial = start(true, "ssh_exec", "starting");
    let completed = poll(&initial, "completed", "other call");
    let host = Host::new(
        initial.clone(),
        vec![Reply::Delayed(
            Duration::from_millis(400),
            completed.clone(),
        )],
        true,
    )
    .await;
    let bridge = host.bridge.clone();
    let blocker = tokio::spawn(async move {
        bridge
            .call(
                tool::TERMINAL_POLL,
                json!({"jobId":"job-1"}),
                CancellationToken::new(),
            )
            .await
            .unwrap()
    });
    host.wait_for(tool::TERMINAL_POLL, 1).await;
    tokio::time::pause();
    let began = Instant::now();
    assert_eq!(
        StartWait(Some(180))
            .wait_for_result(&host.bridge, initial.clone(), CancellationToken::new())
            .await,
        initial
    );
    assert!((Duration::from_millis(180)..=Duration::from_millis(181)).contains(&began.elapsed()));
    tokio::time::resume();
    assert_eq!(blocker.await.unwrap(), completed);
    assert_eq!(host.count(tool::TERMINAL_POLL), 1);
    assert_eq!(host.count("auth"), 1);
    host.finish().await;
}

#[tokio::test]
async fn cancellation_during_backoff_returns_immediately_without_polling() {
    let initial = start(true, "ssh_exec", "starting");
    let host = Host::new(initial.clone(), vec![], true).await;
    tokio::time::pause();
    let cancellation = CancellationToken::new();
    let timer_token = cancellation.clone();
    let timer = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        timer_token.cancel();
    });
    assert_eq!(
        tokio::time::timeout(
            Duration::from_millis(100),
            StartWait::default().wait_for_result(&host.bridge, initial.clone(), cancellation)
        )
        .await
        .unwrap(),
        initial
    );
    timer.await.unwrap();
    assert_eq!(host.count(tool::TERMINAL_POLL), 0);
    host.finish().await;
}

#[tokio::test]
async fn cancellation_during_poll_cancels_rpc_and_keeps_job_result() {
    let initial = start(true, "ssh_exec", "starting");
    let host = Host::new(
        initial.clone(),
        vec![Reply::Delayed(
            Duration::from_secs(10),
            poll(&initial, "completed", "late"),
        )],
        true,
    )
    .await;
    let cancellation = CancellationToken::new();
    let bridge = host.bridge.clone();
    let waiting_token = cancellation.clone();
    let start_value = initial.clone();
    let waiting = tokio::spawn(async move {
        StartWait::default()
            .wait_for_result(&bridge, start_value, waiting_token)
            .await
    });
    host.wait_for(tool::TERMINAL_POLL, 1).await;
    cancellation.cancel();
    assert_eq!(
        tokio::time::timeout(Duration::from_millis(200), waiting)
            .await
            .unwrap()
            .unwrap(),
        initial
    );
    host.wait_for("request.cancel", 1).await;
    assert_eq!(host.count(tool::TERMINAL_START), 0);
    host.finish().await;
}

#[tokio::test]
async fn bridge_deadline_bounds_authentication_before_poll_submission() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let bridge = BridgeClient::new(BridgeEndpoint::for_test(
        listener.local_addr().unwrap().port(),
    ));
    let task = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut lines = BufReader::new(socket).lines();
        assert!(lines.next_line().await.unwrap().is_some());
        assert!(
            lines.next_line().await.unwrap().is_none(),
            "timed out negotiation must close"
        );
    });
    let began = Instant::now();
    let error = bridge
        .call_with_deadline(
            tool::TERMINAL_POLL,
            json!({"jobId":"job-1"}),
            CancellationToken::new(),
            began + Duration::from_millis(30),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "bridge_timeout");
    assert!(began.elapsed() < Duration::from_millis(500));
    task.await.unwrap();
}

#[tokio::test]
async fn deadline_between_polls_returns_last_valid_output() {
    let initial = start(true, "ssh_exec", "starting");
    let running = poll(&initial, "running", "partial output");
    let host = Host::new(initial.clone(), vec![Reply::Value(running.clone())], true).await;
    assert_eq!(
        StartWait(Some(350))
            .wait_for_result(&host.bridge, initial, CancellationToken::new())
            .await,
        running
    );
    assert_eq!(host.count(tool::TERMINAL_POLL), 1);
    host.finish().await;
}

#[tokio::test]
async fn cancelling_status_remains_transient_until_exit() {
    let initial = start(true, "ssh_exec", "starting");
    let cancelling = poll(&initial, "cancelling", "partial output");
    let cancelled = poll(&initial, "cancelled", "final output");
    let host = Host::new(
        initial.clone(),
        vec![Reply::Value(cancelling), Reply::Value(cancelled.clone())],
        true,
    )
    .await;
    assert_eq!(
        StartWait(Some(1000))
            .wait_for_result(&host.bridge, initial, CancellationToken::new())
            .await,
        cancelled
    );
    assert_eq!(host.count(tool::TERMINAL_POLL), 2);
    host.finish().await;
}

#[tokio::test]
async fn terminal_start_wait_ms_returns_completed_in_one_call() {
    let initial = start(true, "ssh_exec", "starting");
    let completed = poll(&initial, "completed", "done!");
    let host = Host::new(
        initial.clone(),
        vec![
            Reply::Value(poll(&initial, "running", "")),
            Reply::Value(completed.clone()),
        ],
        true,
    )
    .await;
    let response = host
        .mcp_call(
            json!({"command":"echo done!", "backend":"ssh_exec", "waitMs":5000,
        "target":{"type":"connection", "connectionId":"c1"}}),
        )
        .await;
    assert_eq!(response["result"]["structuredContent"], completed);
    assert_eq!(host.count(tool::TERMINAL_START), 1);
    assert_eq!(host.count(tool::TERMINAL_POLL), 2);
    host.finish().await;
}

#[tokio::test]
async fn terminal_start_waits_by_default_and_wait_ms_zero_opts_out() {
    let initial = start(true, "ssh_exec", "starting");
    let completed = poll(&initial, "completed", "ok");
    let host = Host::new(initial.clone(), vec![Reply::Value(completed.clone())], true).await;
    let mut arguments =
        json!({"command":"echo test", "target":{"type":"connection", "connectionId":"c1"}});
    let response = host.mcp_call(arguments.clone()).await;
    assert_eq!(response["result"]["structuredContent"], completed);
    arguments["waitMs"] = json!(0);
    let response = host.mcp_call(arguments).await;
    assert_eq!(response["result"]["structuredContent"], initial);
    assert_eq!(host.count(tool::TERMINAL_START), 2);
    assert_eq!(host.count(tool::TERMINAL_POLL), 1);
    host.finish().await;
}
