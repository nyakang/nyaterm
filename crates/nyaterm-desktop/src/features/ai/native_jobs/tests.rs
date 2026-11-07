use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use nyaterm_core::ai::harness::{AgentRequestContext, AgentToolRegistry, AgentToolResult};
use nyaterm_core::ai::{
    AiApiFormat, AiBackendKind, AiChatCompletion, AiChatRequest, AiModelConfigItem, AiModelSource,
    AiProviderApiProtocol, AiProviderCredential, AiProviderKind, AiSettings,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{
    NativeJobEvent, completion_with_repair, run_native_job, warn_agent_protocol_failure,
    warn_native_request_failure,
};
use crate::blocking_jobs::BlockingJobScheduler;
use crate::http::ai::AiProviderSession;
use crate::thread_owner::spawn_joinable;

fn request() -> AiChatRequest {
    let mut request: AiChatRequest = serde_json::from_value(json!({
        "action":"generate_command", "mode":"agent", "userInput":"inspect the environment",
        "sessionId":"chat", "modelId":"mock-model"
    }))
    .unwrap();
    request.options.agent_context = Some(AgentRequestContext {
        run_id: "run".into(),
        turns: Vec::new(),
        remaining_steps: 4,
    });
    request
}

#[tokio::test]
async fn fallback_and_protocol_repair_preserve_the_original_task_language() {
    // Fallback and correction intentionally share one additional POST, so exercise them separately.
    for fallback in [false, true] {
        let initial = if fallback {
            http(
                "400 Bad Request",
                "application/json",
                r#"{"error":{"code":"unsupported_tools"}}"#,
            )
        } else {
            stream("{not JSON", vec![])
        };
        let (base, server) = mock_provider(vec![
            initial,
            stream(r#"{"action":"get_environment","arguments":{}}"#, vec![]),
        ]);
        let mut request = request();
        request.options.language = "zh-CN".into();
        let session = AiProviderSession::new(settings(base), &request).unwrap();
        let (result, _) = run(&session, &mut request, &CancellationToken::new()).await;
        result.unwrap();
        assert_eq!(request.options.language, "zh-CN");
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 2);
        for body in &requests {
            assert!(
                body["messages"][0]["content"]
                    .as_str()
                    .unwrap()
                    .contains("使用简体中文")
            );
        }
        if !fallback {
            assert!(
                requests[1]["messages"][0]["content"]
                    .as_str()
                    .unwrap()
                    .contains("修正后的响应")
            );
        }
    }
}

fn settings(base_url: String) -> AiSettings {
    let mut settings = AiSettings {
        default_model_id: Some("mock-model".into()),
        models: vec![AiModelConfigItem {
            id: "mock-model".into(),
            name: "mock-model".into(),
            backend: AiBackendKind::Genai,
            provider_kind: Some(AiProviderKind::OpenaiCompatible),
            credential_id: Some("mock".into()),
            enabled: true,
            source: AiModelSource::Manual,
            last_seen_at: None,
            supported_reasoning_efforts: None,
        }],
        provider_credentials: vec![AiProviderCredential {
            id: "mock".into(),
            name: "Mock".into(),
            provider_kind: AiProviderKind::OpenaiCompatible,
            api_protocol: Some(AiProviderApiProtocol::OpenaiCompatible),
            api_format: AiApiFormat::ChatCompletions,
            base_url: Some(base_url),
            api_key: Some("fixture-key".into()),
            enabled: true,
            extra_headers: Vec::new(),
            icon_data_url: None,
        }],
        timeout_ms: 3000,
        ..Default::default()
    };
    settings.proxy.mode = nyaterm_core::ai::proxy::AiProxyMode::Direct;
    settings
}

fn http(status: &str, content_type: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn stream(text: &str, calls: Vec<Value>) -> String {
    let delta = if calls.is_empty() {
        json!({"content":text, "reasoning_content":"private-reasoning"})
    } else {
        json!({"tool_calls":calls})
    };
    let body = format!(
        "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
        json!({"choices":[{"index":0,"delta":delta}]}),
        json!({"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]})
    );
    http("200 OK", "text/event-stream", &body)
}

fn tool(index: usize, name: &str, arguments: Value) -> Value {
    json!({"index":index,"id":format!("call-{index}"),"type":"function",
        "function":{"name":name,"arguments":arguments.to_string()}})
}

fn valid_response() -> String {
    stream("", vec![tool(0, "get_environment", json!({}))])
}

// A nonblocking accept loop also records accidental extra POSTs instead of
// hanging forever when a retry-count assertion regresses.
fn mock_provider(replies: Vec<String>) -> (String, thread::JoinHandle<Vec<Value>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let server = spawn_joinable("mock-ai-provider", move || {
        let mut requests = Vec::new();
        let mut deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match listener.accept() {
                Ok((mut connection, _)) => {
                    connection.set_nonblocking(false).unwrap();
                    let body = read_request(&mut connection);
                    requests.push(body);
                    let reply = replies.get(requests.len() - 1).cloned().unwrap_or_else(|| {
                        http("500 Internal Server Error", "application/json", "{}")
                    });
                    connection.write_all(reply.as_bytes()).unwrap();
                    if requests.len() >= replies.len() {
                        deadline = Instant::now() + Duration::from_millis(150);
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        break;
                    }
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("mock accept failed: {error}"),
            }
        }
        requests
    })
    .expect("spawn mock provider");
    (base, server)
}

fn read_request(connection: &mut TcpStream) -> Value {
    connection
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut bytes = Vec::new();
    let mut chunk = [0; 4096];
    loop {
        let count = connection.read(&mut chunk).unwrap();
        assert_ne!(count, 0);
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&bytes[..end]);
            let length = headers
                .lines()
                .find_map(|line| {
                    let (key, value) = line.split_once(':')?;
                    key.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap();
            if bytes.len() >= end + 4 + length {
                return serde_json::from_slice(&bytes[end + 4..end + 4 + length]).unwrap();
            }
        }
    }
}

async fn run(
    session: &Arc<AiProviderSession>,
    request: &mut AiChatRequest,
    cancel: &CancellationToken,
) -> (Result<AiChatCompletion, String>, Vec<NativeJobEvent>) {
    let (tx, mut rx) = mpsc::channel(64);
    let work = async {
        let result = completion_with_repair(session, request, &[], &tx, cancel).await;
        drop(tx);
        result
    };
    let consume = async {
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        events
    };
    tokio::join!(work, consume)
}

#[tokio::test]
async fn invalid_text_is_corrected_once_with_the_original_tool_transcript() {
    let (base, server) = mock_provider(vec![
        stream("{private-rejected-response", vec![]),
        valid_response(),
    ]);
    let mut request = request();
    let call = AgentToolRegistry::parse(
        &[],
        r#"{"action":"session_get","arguments":{"sessionId":"terminal"}}"#,
    )
    .unwrap();
    let result = AgentToolResult {
        call_id: call.id.clone(),
        value: json!({"fixture":"previous-result"}),
        is_error: false,
    };
    request
        .options
        .agent_context
        .as_mut()
        .unwrap()
        .turns
        .push((call, result).into());
    let original_context = request.options.agent_context.clone();
    let original_input = request.user_input.clone();
    let session = AiProviderSession::new(settings(base), &request).unwrap();
    let (result, events) = run(&session, &mut request, &CancellationToken::new()).await;
    assert_eq!(result.unwrap().tool_calls[0].name, "get_environment");
    assert_eq!(request.options.agent_context, original_context);
    assert_eq!(request.user_input, original_input);
    assert!(!request.options.agent_json_protocol);
    let repair = events
        .iter()
        .position(|event| matches!(event, NativeJobEvent::ProtocolRepair))
        .unwrap();
    assert!(events[..repair].iter().any(|event| matches!(event, NativeJobEvent::Delta(delta) if delta.text_delta == "{private-rejected-response")));
    assert!(events[repair + 1..].iter().all(|event| !matches!(event, NativeJobEvent::Delta(delta) if !delta.text_delta.is_empty() || delta.reasoning_delta.is_some())));
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[1]["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("No tool was executed")
    );
    assert!(requests[1].to_string().contains("previous-result"));
    assert!(
        !requests[1]
            .to_string()
            .contains("{private-rejected-response")
    );
    assert!(!requests[1].to_string().contains("private-reasoning"));
    assert_eq!(
        requests[0]["messages"].as_array().unwrap()[1..],
        requests[1]["messages"].as_array().unwrap()[1..]
    );
    assert!(requests[1].get("parallel_tool_calls").is_none());
}

#[tokio::test]
async fn repeated_invalid_responses_stop_after_two_posts() {
    let (base, server) = mock_provider(vec![
        stream("{private-rejected-response", vec![]),
        stream("{private-second-response", vec![]),
    ]);
    let mut request = request();
    let session = AiProviderSession::new(settings(base), &request).unwrap();
    let (result, events) = run(&session, &mut request, &CancellationToken::new()).await;
    let error = result.unwrap_err();
    assert!(error.contains("malformed JSON action"));
    assert!(!error.contains("private-"));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, NativeJobEvent::ProtocolRepair))
            .count(),
        1
    );
    assert_eq!(server.join().unwrap().len(), 2);
}

#[tokio::test]
async fn unsupported_tools_fallback_consumes_the_same_extra_request_budget() {
    let unsupported = http(
        "400 Bad Request",
        "application/json",
        r#"{"error":{"code":"unsupported_tools"}}"#,
    );
    let (base, server) = mock_provider(vec![unsupported, stream("{not JSON", vec![])]);
    let mut request = request();
    let session = AiProviderSession::new(settings(base), &request).unwrap();
    let (result, events) = run(&session, &mut request, &CancellationToken::new()).await;
    assert!(result.unwrap_err().contains("malformed JSON action"));
    assert!(request.options.agent_json_protocol);
    assert!(!request.options.agent_protocol_repair);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, NativeJobEvent::ProtocolFallback))
            .count(),
        1
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, NativeJobEvent::ProtocolRepair))
    );
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests[1].get("tools").is_none());
}

#[tokio::test]
async fn json_protocol_correction_keeps_json_protocol() {
    let (base, server) = mock_provider(vec![
        stream("{not JSON", vec![]),
        stream(r#"{"action":"get_environment","arguments":{}}"#, vec![]),
    ]);
    let mut request = request();
    request.options.agent_json_protocol = true;
    let session = AiProviderSession::new(settings(base), &request).unwrap();
    let (result, _) = run(&session, &mut request, &CancellationToken::new()).await;
    let completion = result.unwrap();
    assert!(AgentToolRegistry::parse(&completion.tool_calls, &completion.text).is_ok());
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests[1].get("tools").is_none());
    assert!(
        requests[1]["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("Return exactly one JSON action")
    );
}

#[tokio::test]
async fn other_protocol_and_provider_failures_do_not_retry() {
    let cases = vec![
        stream("", vec![]),
        stream("partial answer", vec![]).replace("\"stop\"", "\"length\""),
        http(
            "400 Bad Request",
            "application/json",
            r#"{"error":{"code":"unsupported_parameter","param":"reasoning_effort","message":"private-body"}}"#,
        ),
        http("401 Unauthorized", "application/json", "{}"),
        http("403 Forbidden", "application/json", "{}"),
        http("429 Too Many Requests", "application/json", "{}"),
        http("500 Internal Server Error", "application/json", "{}"),
        // A partial SSE stream is a transport failure, not a completed bad action.
        http(
            "200 OK",
            "text/event-stream",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"partial\"}}]}\n\n",
        ),
    ];
    for reply in cases {
        let (base, server) = mock_provider(vec![reply]);
        let mut request = request();
        let session = AiProviderSession::new(settings(base), &request).unwrap();
        let (result, events) = run(&session, &mut request, &CancellationToken::new()).await;
        assert!(result.is_err());
        assert!(!events.iter().any(|event| matches!(
            event,
            NativeJobEvent::ProtocolRepair | NativeJobEvent::ProtocolFallback
        )));
        assert_eq!(server.join().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn completed_text_and_tool_batches_dispatch_without_protocol_retries() {
    for calls in [
        vec![],
        vec![
            tool(0, "get_environment", json!({})),
            tool(1, "session_get", json!({"sessionId":"s"})),
        ],
        vec![
            tool(0, "terminal_execute", json!({"command":42,"sessionId":"s"})),
            tool(1, "get_environment", json!({})),
        ],
    ] {
        let (base, server) = mock_provider(vec![stream("Explanation", calls.clone())]);
        let mut request = request();
        let session = AiProviderSession::new(settings(base), &request).unwrap();
        let (result, events) = run(&session, &mut request, &CancellationToken::new()).await;
        let decision = AgentToolRegistry::decision(&result.unwrap()).unwrap();
        match decision {
            nyaterm_core::ai::harness::AgentDecision::FinalText(text) => {
                assert!(calls.is_empty());
                assert_eq!(text, "Explanation");
            }
            nyaterm_core::ai::harness::AgentDecision::Tools(batch) => {
                assert_eq!(batch.calls.len(), 2);
                assert_eq!(
                    batch.calls[0].action.is_err(),
                    calls[0]["function"]["name"] == "terminal_execute"
                );
            }
        }
        assert!(!events.iter().any(|event| matches!(
            event,
            NativeJobEvent::ProtocolRepair | NativeJobEvent::ProtocolFallback
        )));
        assert_eq!(server.join().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn unsupported_tools_after_a_completed_turn_cannot_change_protocol_or_replay_tools() {
    let (base, server) = mock_provider(vec![http(
        "400 Bad Request",
        "application/json",
        r#"{"error":{"code":"unsupported_tools"}}"#,
    )]);
    let mut request = request();
    let call =
        AgentToolRegistry::parse(&[], r#"{"action":"get_environment","arguments":{}}"#).unwrap();
    let result = AgentToolResult {
        call_id: call.id.clone(),
        value: json!({"alreadyExecuted":true}),
        is_error: false,
    };
    request
        .options
        .agent_context
        .as_mut()
        .unwrap()
        .turns
        .push((call, result).into());
    let original = request.options.agent_context.clone();
    let session = AiProviderSession::new(settings(base), &request).unwrap();
    let (result, events) = run(&session, &mut request, &CancellationToken::new()).await;
    assert!(result.is_err());
    assert_eq!(request.options.agent_context, original);
    assert!(!request.options.agent_json_protocol);
    assert!(!events.iter().any(|event| matches!(
        event,
        NativeJobEvent::ProtocolRepair | NativeJobEvent::ProtocolFallback
    )));
    assert_eq!(server.join().unwrap().len(), 1);
}

#[tokio::test]
async fn cancellation_while_queued_at_the_retry_boundary_prevents_another_post() {
    let (base, server) = mock_provider(vec![stream("{private-response", vec![])]);
    let mut request = request();
    let session = AiProviderSession::new(settings(base), &request).unwrap();
    // Text, reasoning and terminal metadata fill this queue. Cancel at the
    // correction boundary while backpressure prevents its notification.
    let (tx, mut rx) = mpsc::channel(1);
    let cancel = CancellationToken::new();
    let work = completion_with_repair(&session, &mut request, &[], &tx, &cancel);
    let control = async {
        while let Some(event) = rx.recv().await {
            if matches!(event, NativeJobEvent::Delta(delta) if delta.done) {
                cancel.cancel();
                break;
            }
        }
    };
    let (result, _) = tokio::join!(work, control);
    assert!(result.unwrap_err().contains("cancelled"));
    assert_eq!(server.join().unwrap().len(), 1);
}

#[derive(Clone, Default)]
struct DiagnosticCapture(Arc<Mutex<Vec<u8>>>);

#[test]
fn protocol_failure_diagnostics_omit_model_content_and_tool_arguments() {
    use nyaterm_core::ai::AiToolCall;
    use nyaterm_core::ai::harness::AgentResponseError;

    let capture = DiagnosticCapture::default();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_ansi(false)
        .without_time()
        .with_writer(capture.clone())
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        let mut completion = AiChatCompletion {
            text: "{private-response".into(),
            reasoning_content: Some("private-reasoning".into()),
            tool_calls: Vec::new(),
            metadata: Default::default(),
        };
        warn_agent_protocol_failure(&completion, 1, &AgentResponseError::InvalidJsonAction);
        warn_agent_protocol_failure(&completion, 2, &AgentResponseError::InvalidJsonAction);
        completion.tool_calls.push(AiToolCall {
            name: "private-tool-name".into(),
            id: Some("private-id".into()),
            thought_signature: Some("private-signature".into()),
            arguments: json!({"command":"private-command","password":"private-password"}),
        });
        warn_agent_protocol_failure(&completion, 1, &AgentResponseError::InvalidArguments);
        let mut req = request();
        req.user_input = "private-user-input".into();
        req.model_name = Some("private-model".into());
        let mut error = nyaterm_core::ai::completion::AiProviderError::new(
            nyaterm_core::ai::completion::AiProviderErrorKind::Protocol,
        );
        error.failure_stage =
            Some(nyaterm_core::ai::completion::AiProviderFailureStage::DecodeStream);
        error.stream = Some(nyaterm_core::ai::completion::AiStreamDiagnostic {
            frames: 3,
            terminal: false,
            failed: true,
            tool_count: 1,
            failure: None,
        });
        warn_native_request_failure(&error, &req, 1);
    });
    let logs = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
    assert_eq!(logs.matches("malformed_json_action").count(), 2, "{logs}");
    assert!(logs.contains("invalid_tool_arguments"));
    assert!(logs.contains("attempt=1"));
    assert!(logs.contains("attempt=2"));
    assert!(logs.contains("tool_count=0"));
    assert!(logs.contains("tool_count=1"));
    assert!(logs.contains("AI native request failed"));
    assert!(logs.contains("DecodeStream"));
    assert!(!logs.contains("private-"));
}

#[tokio::test]
async fn correction_prepares_and_persists_the_user_message_only_once() {
    use nyaterm_core::ai::AiMessageRole;
    use nyaterm_store::StoreDomain;

    let root = nyaterm_core::test_support::TestTempDir::new("nyaterm-agent-protocol-repair");
    let store = crate::test_support::blocking_test_store(root.path());
    let jobs = BlockingJobScheduler::with_limits(1, 8).unwrap();
    let (base, server) = mock_provider(vec![stream("{invalid-response", vec![]), valid_response()]);
    let request = request();
    let mut config = settings(base);
    config.record_history = true;
    let session = AiProviderSession::new(config, &request).unwrap();
    let cancel = CancellationToken::new();
    let (tx, mut rx) = mpsc::channel(64);
    let work = async {
        let result = run_native_job(
            jobs.clone(),
            store.clone(),
            session,
            request,
            Some(Vec::new()),
            true,
            &tx,
            &cancel,
        )
        .await;
        tx.send(NativeJobEvent::Finished(Box::new(result)))
            .await
            .ok();
        drop(tx);
    };
    let consume = async {
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        events
    };
    let (_, mut events) = tokio::join!(work, consume);
    let Some(NativeJobEvent::Finished(result)) = events.pop() else {
        panic!("finished event must be last")
    };
    assert_eq!(
        match result.unwrap().native_decision.unwrap() {
            nyaterm_core::ai::harness::AgentDecision::Tools(batch) =>
                batch.calls[0].action.as_ref().unwrap().tool.name(),
            _ => panic!("expected tools"),
        },
        "get_environment"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, NativeJobEvent::ProtocolRepair))
            .count(),
        1
    );
    let history = store
        .request_fn(StoreDomain::Ai, |database| database.load_ai_history())
        .unwrap();
    assert_eq!(history.messages.len(), 1);
    assert_eq!(history.messages[0].role, AiMessageRole::User);
    assert_eq!(history.messages[0].content, "inspect the environment");
    assert_eq!(server.join().unwrap().len(), 2);
    jobs.shutdown();
}

#[tokio::test]
async fn ask_and_legacy_agent_responses_are_not_repaired() {
    for mode in [
        nyaterm_core::ai::AiMode::Ask,
        nyaterm_core::ai::AiMode::Agent,
    ] {
        let (base, server) = mock_provider(vec![stream("ordinary text", vec![])]);
        let mut request = request();
        request.mode = mode;
        request.options.agent_context = None;
        let session = AiProviderSession::new(settings(base), &request).unwrap();
        let (result, events) = run(&session, &mut request, &CancellationToken::new()).await;
        assert_eq!(result.unwrap().text, "ordinary text");
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, NativeJobEvent::ProtocolRepair))
        );
        assert_eq!(server.join().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn connection_failure_and_pre_cancelled_requests_do_not_repair() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    drop(listener);
    for cancelled in [false, true] {
        let mut request = request();
        let session = AiProviderSession::new(settings(base.clone()), &request).unwrap();
        let cancel = CancellationToken::new();
        if cancelled {
            cancel.cancel();
        }
        let (result, events) = run(&session, &mut request, &cancel).await;
        assert!(result.is_err());
        assert!(events.is_empty());
        assert!(!request.options.agent_protocol_repair);
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for DiagnosticCapture {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

impl Write for DiagnosticCapture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
