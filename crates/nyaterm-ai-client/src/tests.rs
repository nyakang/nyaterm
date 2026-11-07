use super::{AiProviderSession, complete_chat, stream_chat};
use nyaterm_core::ai::completion::AiProviderErrorKind;
use nyaterm_core::ai::{
    AiApiFormat, AiBackendKind, AiMode, AiModelConfigItem, AiModelSource, AiProviderApiProtocol,
    AiProviderCredential, AiProviderKind, AiReasoningEffort, AiSettings,
};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
fn responses_settings(base_url: String) -> AiSettings {
    let credential = AiProviderCredential {
        icon_data_url: None,
        api_protocol: None,
        id: "mock-responses".to_string(),
        name: "Mock Responses".to_string(),
        provider_kind: AiProviderKind::OpenaiCompatible,
        api_format: AiApiFormat::Responses,
        base_url: Some(base_url),
        api_key: Some("mock-fixture-key".to_string().into()),
        extra_headers: Vec::new(),
        enabled: true,
    };
    let model = AiModelConfigItem {
        supported_reasoning_efforts: None,
        id: "mock-responses:gpt-test".to_string(),
        name: "gpt-test".to_string(),
        backend: AiBackendKind::Genai,
        provider_kind: Some(AiProviderKind::OpenaiCompatible),
        credential_id: Some(credential.id.clone()),
        enabled: true,
        source: AiModelSource::Manual,
        last_seen_at: None,
    };
    AiSettings {
        default_model_id: Some(model.id.clone()),
        models: vec![model],
        provider_credentials: vec![credential],
        default_reasoning_effort: AiReasoningEffort::High,
        ..AiSettings::default()
    }
}

fn request() -> nyaterm_core::AiChatRequest {
    nyaterm_core::AiChatRequest {
        owner_scope: Default::default(),
        targets: Vec::new(),
        target_contexts: Vec::new(),
        agent_kind: Default::default(),
        permission_mode: Default::default(),
        default_target_session_id: None,
        existing_external_session_id: None,
        attachments: Vec::new(),
        stream_id: Some("stream-mock".to_string()),
        session_id: Some("session-mock".to_string()),
        connection_id: None,
        terminal_session_id: None,
        mode: AiMode::Ask,
        model_id: Some("mock-responses:gpt-test".to_string()),
        model_name: None,
        action: nyaterm_core::AiAction::GenerateCommand,
        user_input: "show disk usage".to_string(),
        context: Default::default(),
        options: Default::default(),
    }
}

fn mock_server(
    response_body: String,
    content_type: &'static str,
) -> (String, thread::JoinHandle<String>) {
    mock_server_status(response_body, content_type, "200 OK")
}
fn mock_server_status(
    response_body: String,
    content_type: &'static str,
    status: &'static str,
) -> (String, thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
    let address = listener.local_addr().expect("mock address");
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept request");
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        stream
            .set_write_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let request = read_http_request(&mut stream);
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
            response_body.len()
        );
        stream
            .write_all(response.as_bytes())
            .expect("write response");
        request
    });
    (format!("http://{address}/v1"), handle)
}

fn read_http_request(stream: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 2048];
    let mut expected_len = None;
    loop {
        let count = stream.read(&mut chunk).expect("read request");
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            let header_end = header_end + 4;
            let headers = String::from_utf8_lossy(&bytes[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or_default();
            expected_len = Some(header_end + content_length);
        }
        if expected_len.is_some_and(|expected| bytes.len() >= expected) {
            break;
        }
    }
    String::from_utf8(bytes).expect("UTF-8 HTTP request")
}

fn settings(base: String, protocol: AiProviderApiProtocol) -> AiSettings {
    let mut settings = responses_settings(base);
    settings.provider_credentials[0].api_format = AiApiFormat::ChatCompletions;
    settings.provider_credentials[0].api_protocol = Some(protocol);
    settings.default_reasoning_effort = AiReasoningEffort::Auto;
    settings.proxy.mode = nyaterm_core::ai::proxy::AiProxyMode::Direct;
    settings
}
fn body(raw: &str) -> Value {
    serde_json::from_str(raw.split_once("\r\n\r\n").unwrap().1).unwrap()
}
fn sse(values: &[Value], done: bool) -> String {
    let mut result = values
        .iter()
        .map(|value| {
            format!(
                "{}data: {value}\n\n",
                value
                    .get("type")
                    .and_then(Value::as_str)
                    .map(|kind| format!("event: {kind}\n"))
                    .unwrap_or_default()
            )
        })
        .collect::<String>();
    if done {
        result.push_str("data: [DONE]\n\n");
    }
    result
}
async fn streamed(
    settings: AiSettings,
    request: nyaterm_core::ai::AiChatRequest,
) -> (
    Result<nyaterm_core::ai::AiChatCompletion, nyaterm_core::ai::completion::AiProviderError>,
    Vec<nyaterm_core::ai::AiChatStreamDelta>,
) {
    let session = AiProviderSession::new(settings, &request).unwrap();
    let (tx, mut rx) = mpsc::channel(64);
    let cancel = CancellationToken::new();
    let consume = async move {
        let mut deltas = Vec::new();
        while let Some(delta) = rx.recv().await {
            deltas.push(delta);
        }
        deltas
    };
    tokio::join!(stream_chat(&session, &request, &[], tx, &cancel), consume)
}

#[tokio::test]
async fn openai_accounts_headers_query_and_auto_preserve_wire_contract() {
    for anonymous in [false, true] {
        let (base, server) = mock_server(json!({"id":"reply-id","model":"gpt-test-none","choices":[{"message":{"role":"assistant","content":"你好"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}}).to_string(), "application/json");
        let mut config = settings(
            format!("{base}?tenant=fixture"),
            AiProviderApiProtocol::OpenaiCompatible,
        );
        config.models[0].name = "gpt-test-none".into();
        config.provider_credentials[0].api_key = (!anonymous).then(|| "unused-secret".into());
        if !anonymous {
            config.provider_credentials[0].extra_headers = vec![nyaterm_core::AiHttpHeader {
                id: "h".into(),
                name: "aUtHoRiZaTiOn".into(),
                value: "Bearer override-secret".into(),
            }];
        }
        let session = AiProviderSession::new(config, &request()).unwrap();
        let result = complete_chat(&session, &request(), &[], &CancellationToken::new())
            .await
            .unwrap();
        let raw = server.join().unwrap();
        assert!(raw.starts_with("POST /v1/chat/completions?tenant=fixture "));
        assert_eq!(
            raw.to_lowercase().matches("authorization:").count(),
            usize::from(!anonymous)
        );
        assert!(!raw.contains("unused-secret"));
        let payload = body(&raw);
        assert_eq!(payload["model"], "gpt-test-none");
        assert!(payload.get("reasoning_effort").is_none());
        assert_eq!(result.text, "你好");
        assert_eq!(result.metadata.usage.total_tokens, Some(5));
        assert_eq!(result.metadata.response_id.as_deref(), Some("reply-id"));
        assert!(!format!("{result:?}").contains("你好"));
    }
}

#[tokio::test]
async fn stream_keeps_interleaved_sparse_tool_calls_and_finishes_once() {
    let frames = sse(
        &[
            json!({"id":"r","choices":[{"index":0,"delta":{"content":"检查","reasoning_content":"思考","tool_calls":[{"index":2,"id":"a","type":"function","function":{"name":"first","arguments":"{\"x\":"}},{"index":7,"id":"b","type":"function","function":{"name":"second","arguments":"{\"y\":"}}]}}]}),
            json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":7,"function":{"arguments":"2}"}},{"index":2,"function":{"arguments":"1}"}}]}}]}),
            json!({"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":7,"completion_tokens":3,"total_tokens":10}}),
        ],
        true,
    );
    let (base, server) = mock_server(frames, "text/event-stream");
    let (result, deltas) = streamed(
        settings(base, AiProviderApiProtocol::OpenaiCompatible),
        request(),
    )
    .await;
    server.join().unwrap();
    let result = result.unwrap();
    assert_eq!(result.text, "检查");
    assert_eq!(result.reasoning_content.as_deref(), Some("思考"));
    assert_eq!(result.tool_calls.len(), 2);
    assert_eq!(deltas.iter().filter(|delta| delta.done).count(), 1);
    assert_eq!(
        deltas
            .iter()
            .map(|delta| delta.text_delta.as_str())
            .collect::<String>(),
        "检查"
    );
    for (index, expected) in [(2, json!({"x":1})), (7, json!({"y":2}))] {
        let arguments = deltas
            .iter()
            .flat_map(|delta| &delta.tool_call_deltas)
            .filter(|call| call.index == index)
            .map(|call| call.arguments_delta.as_str())
            .collect::<String>();
        assert_eq!(serde_json::from_str::<Value>(&arguments).unwrap(), expected);
    }
}

#[tokio::test]
async fn gemini_auto_preserves_literal_model_without_suffix_inference() {
    let (base, server) = mock_server(
        json!({"candidates":[{"content":{"role":"model","parts":[{"text":"ok"}]},"finishReason":"STOP"}]}).to_string(),
        "application/json",
    );
    let mut config = settings(base, AiProviderApiProtocol::Gemini);
    config.models[0].name = "gemini-custom-high".into();
    let session = AiProviderSession::new(config, &request()).unwrap();
    complete_chat(&session, &request(), &[], &CancellationToken::new())
        .await
        .unwrap();
    let raw = server.join().unwrap();
    assert!(raw.contains("gemini-custom-high:generateContent"));
    assert_eq!(
        body(&raw).pointer("/generationConfig/thinkingConfig"),
        Some(&json!({"includeThoughts":true}))
    );
}

#[tokio::test]
async fn responses_stream_failures_report_safe_stage_and_frame_counts() {
    use nyaterm_core::ai::completion::{
        AiProviderDiagnosticCode as Code, AiProviderFailureStage as Stage,
    };

    let item = json!({"type":"function_call","id":"private-item","call_id":"private-call","name":"session_get","arguments":""});
    let frames = [
        json!({"type":"response.output_item.added","output_index":0,"item":item}),
        json!({"type":"response.function_call_arguments.delta","output_index":0,"item_id":"private-item","delta":"{\"sessionId\":\"private-session\"}"}),
        json!({"type":"error","code":"private-code","message":"private-provider-message"}),
    ];
    let (base, server) = mock_server(sse(&frames, false), "text/event-stream");
    let (result, _) = streamed(responses_settings(base), request()).await;
    server.join().unwrap();
    let error = result.unwrap_err();
    assert_eq!(error.kind, AiProviderErrorKind::Protocol);
    assert_eq!(error.failure_stage, Some(Stage::DecodeStream));
    assert_eq!(error.diagnostic_code, Some(Code::ProviderStreamError));
    assert!(error.responses);
    let stream = error.stream.unwrap();
    assert_eq!(stream.frames, 3);
    assert!(!stream.terminal && stream.failed);
    assert_eq!(stream.tool_count, 1);
    let failure = stream.failure.unwrap();
    assert_eq!(
        failure.reason,
        nyaterm_core::ai::completion::AiStreamFrameFailureReason::ProviderError
    );
    assert_eq!(failure.frame, 3);
    assert!(!format!("{error:?} {error}").contains("private"));
}

#[tokio::test]
async fn responses_empty_sse_blocks_between_content_frames_do_not_reject_completion() {
    // Match the reported failure: three content frames, then an empty fourth
    // frame. genai taps comment-only blocks before skipping their empty data.
    for keepalive in [
        ": keep-alive\n\n",
        "event: ping\n\n",
        "data:\n\n",
        "data:   \n\n",
        "id: fixture\nretry: 1000\n\n",
    ] {
        let mut frames = sse(
            &[
                json!({"type":"response.created"}),
                json!({"type":"response.reasoning_summary_text.delta","delta":"Thinking"}),
                json!({"type":"response.output_text.delta","delta":"Done"}),
            ],
            false,
        );
        frames.push_str(keepalive);
        frames.push_str(&sse(&[json!({"type":"response.completed","response":{"id":"response-fixture","object":"response","model":"gpt-test","status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Done"}]}]}})], false));
        let (base, server) = mock_server(frames, "text/event-stream");
        let (result, deltas) = streamed(responses_settings(base), request()).await;
        server.join().unwrap();
        let result = result.unwrap();
        assert_eq!(result.text, "Done");
        assert_eq!(result.reasoning_content.as_deref(), Some("Thinking"));
        assert_eq!(deltas.iter().filter(|delta| delta.done).count(), 1);
    }
}

#[tokio::test]
async fn responses_empty_sse_blocks_do_not_hide_truncation_or_malformed_data() {
    use nyaterm_core::ai::completion::AiStreamFrameFailureReason;

    for malformed in [false, true] {
        let mut frames = sse(
            &[json!({"type":"response.output_text.delta","delta":"Done"})],
            false,
        );
        frames.push_str(": keep-alive\n\ndata:\n\n");
        if malformed {
            frames.push_str("data: private-invalid-json\n\n");
            frames.push_str(&sse(&[json!({"type":"response.completed","response":{"id":"response-fixture","object":"response","model":"gpt-test","status":"completed","output":[]}})], false));
        }
        let (base, server) = mock_server(frames, "text/event-stream");
        let (result, deltas) = streamed(responses_settings(base), request()).await;
        server.join().unwrap();
        let error = result.unwrap_err();
        let stream = error.stream.unwrap();
        if malformed {
            assert_eq!(error.kind, AiProviderErrorKind::Protocol);
            let failure = stream.failure.unwrap();
            assert_eq!(failure.reason, AiStreamFrameFailureReason::InvalidJson);
            assert_eq!(failure.frame, 4);
            assert_eq!(failure.data_bytes, "private-invalid-json".len());
        } else {
            assert_eq!(error.kind, AiProviderErrorKind::StreamInterrupted);
            assert!(!stream.terminal && !stream.failed);
            assert!(stream.failure.is_none());
        }
        assert!(!deltas.iter().any(|delta| delta.done));
        assert!(!format!("{error:?} {error}").contains("private"));
    }
}

#[tokio::test]
async fn responses_nullable_error_metadata_can_finish_after_many_generated_frames() {
    let mut frames = vec![
        json!({"type":"response.reasoning_summary_text.delta","delta":"private-reasoning","error":null});
        102
    ];
    frames.push(json!({"type":"response.output_text.delta","delta":"Done","error":null}));
    frames.push(json!({"type":"response.completed","error":null,"response":{"id":"response-fixture","object":"response","model":"gpt-test","status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Done"}]}]}}));
    let (base, server) = mock_server(sse(&frames, false), "text/event-stream");
    let (result, deltas) = streamed(responses_settings(base), request()).await;
    server.join().unwrap();
    assert_eq!(result.unwrap().text, "Done");
    assert_eq!(deltas.iter().filter(|delta| delta.done).count(), 1);
}

#[tokio::test]
async fn responses_rejected_item_reports_the_first_bad_frame_without_payloads() {
    use nyaterm_core::ai::completion::{
        AiProviderDiagnosticCode, AiStreamEventKind, AiStreamFrameFailureReason,
    };
    let frames = [
        json!({"type":"response.output_text.delta","delta":"private-text"}),
        json!({"type":"response.output_item.done","item":null,"private-field":"private-content"}),
        json!({"type":"response.completed","response":{"id":"response-fixture","object":"response","model":"gpt-test","status":"completed","output":[]}}),
    ];
    let (base, server) = mock_server(sse(&frames, false), "text/event-stream");
    let (result, _) = streamed(responses_settings(base), request()).await;
    server.join().unwrap();
    let error = result.unwrap_err();
    assert_eq!(
        error.diagnostic_code,
        Some(AiProviderDiagnosticCode::InvalidStreamFrame)
    );
    let failure = error.stream.unwrap().failure.unwrap();
    assert_eq!(failure.frame, 2);
    assert_eq!(
        failure.reason,
        AiStreamFrameFailureReason::MissingOutputItemObject
    );
    assert_eq!(failure.event, AiStreamEventKind::OutputItemDone);
    assert!(!format!("{error:?} {error}").contains("private"));
}

#[test]
fn sdk_generation_errors_classify_without_exposing_cause_or_payloads() {
    use nyaterm_core::ai::completion::AiProviderDiagnosticCode as Code;
    let error = super::error::sdk_error(genai::Error::ChatResponseGeneration {
        model_iden: genai::ModelIden::new(genai::adapter::AdapterKind::OpenAI, "private-model"),
        request_payload: Box::new(json!({"password":"private-password"})),
        response_body: Box::new(json!({"content":"private-body"})),
        cause: "private-cause".into(),
    });
    assert_eq!(error.diagnostic_code, Some(Code::SdkResponseGeneration));
    assert!(!format!("{error:?} {error}").contains("private"));
}

#[tokio::test]
async fn malformed_arguments_and_unterminated_streams_fail_closed() {
    for (arguments, done, expected) in [
        ("{\"x\":", true, AiProviderErrorKind::Protocol),
        ("{}", false, AiProviderErrorKind::StreamInterrupted),
    ] {
        let (base, server) = mock_server(
            sse(
                &[
                    json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"a","function":{"name":"first","arguments":arguments}}]}}]}),
                ],
                done,
            ),
            "text/event-stream",
        );
        let (result, deltas) = streamed(
            settings(base, AiProviderApiProtocol::OpenaiCompatible),
            request(),
        )
        .await;
        server.join().unwrap();
        assert_eq!(result.unwrap_err().kind, expected);
        assert!(!deltas.iter().any(|delta| delta.done));
    }
}

#[tokio::test]
async fn errors_keep_status_and_retry_after_without_provider_diagnostics() {
    for (status, expected) in [
        ("401 Unauthorized", AiProviderErrorKind::Authentication),
        ("403 Forbidden", AiProviderErrorKind::Authentication),
        ("429 Too Many Requests", AiProviderErrorKind::RateLimit),
        ("500 Internal Server Error", AiProviderErrorKind::Server),
    ] {
        let (base, server) = mock_server_status(
            "{\"error\":{\"message\":\"secret-input\"}}".into(),
            "application/json",
            status,
        );
        let config = settings(base, AiProviderApiProtocol::OpenaiCompatible);
        let session = AiProviderSession::new(config, &request()).unwrap();
        let error = complete_chat(&session, &request(), &[], &CancellationToken::new())
            .await
            .unwrap_err();
        server.join().unwrap();
        assert_eq!(error.kind, expected);
        assert!(!error.to_string().contains("secret-input"));
    }
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("retry-after", "17".parse().unwrap());
    assert_eq!(
        super::error::status_error(429, &headers, None).retry_after,
        Some(std::time::Duration::from_secs(17))
    );
}

#[tokio::test]
async fn cancellation_interrupts_full_queue_and_utf8_chunks_stay_bounded() {
    let large = "中".repeat(500_000);
    let (base, server) = mock_server(
        sse(
            &[json!({"choices":[{"index":0,"delta":{"content":large}}]})],
            true,
        ),
        "text/event-stream",
    );
    let config = settings(base, AiProviderApiProtocol::OpenaiCompatible);
    let req = request();
    let session = AiProviderSession::new(config, &req).unwrap();
    let (tx, mut rx) = mpsc::channel(64);
    let cancel = CancellationToken::new();
    let control = async {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while rx.len() < 64 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("producer fills the bounded queue");
        cancel.cancel();
    };
    let (result, _) = tokio::join!(stream_chat(&session, &req, &[], tx, &cancel), control);
    server.join().unwrap();
    assert_eq!(result.unwrap_err().kind, AiProviderErrorKind::Cancelled);
    assert!(rx.len() <= 64);
    while let Some(delta) = rx.recv().await {
        assert!(delta.text_delta.len() <= 16384);
        assert!(delta.text_delta.chars().all(|ch| ch == '中'));
    }
}

fn continuation(
    mut req: nyaterm_core::ai::AiChatRequest,
    completion: nyaterm_core::ai::AiChatCompletion,
) -> nyaterm_core::ai::AiChatRequest {
    use nyaterm_core::ai::harness::{AgentRequestContext, AgentToolRegistry, AgentToolResult};
    let nyaterm_core::ai::harness::AgentDecision::Tools(batch) =
        AgentToolRegistry::decision(&completion).unwrap()
    else {
        panic!("expected tools");
    };
    let results = batch
        .calls
        .iter()
        .map(|entry| AgentToolResult {
            call_id: entry.source.id.clone().unwrap(),
            value: json!({"denied":true}),
            is_error: true,
        })
        .collect();
    req.options.agent_context = Some(AgentRequestContext {
        run_id: "run".into(),
        turns: vec![nyaterm_core::ai::harness::AgentTurnRecord { batch, results }],
        remaining_steps: 4,
    });
    req
}

#[tokio::test]
async fn anthropic_signed_redacted_blocks_and_error_results_round_trip() {
    let blocks = json!([
        {"type":"thinking","thinking":"reason one","signature":"signature-one","opaque":"keep"},
        {"type":"redacted_thinking","data":"encrypted-block"},
        {"type":"thinking","thinking":"reason two","signature":"signature-two"},
        {"type":"tool_use","id":"tool-a","name":"session_get","input":{"sessionId":"terminal-a"}}
    ]);
    let reply = json!({"id":"message-a","type":"message","role":"assistant","model":"claude-test","content":blocks,"stop_reason":"tool_use","usage":{"input_tokens":10,"output_tokens":20}});
    let (base, server) = mock_server(reply.to_string(), "application/json");
    let config = settings(base, AiProviderApiProtocol::Anthropic);
    let session = AiProviderSession::new(config, &request()).unwrap();
    let completion = complete_chat(&session, &request(), &[], &CancellationToken::new())
        .await
        .unwrap();
    server.join().unwrap();
    assert_eq!(
        completion.metadata.assistant_turn.provider_items,
        blocks.as_array().unwrap().clone()
    );
    let req = continuation(request(), completion);
    let (base, server) = mock_server(json!({"id":"message-b","type":"message","role":"assistant","content":[{"type":"text","text":"ok"}],"usage":{"input_tokens":1,"output_tokens":1},"stop_reason":"end_turn"}).to_string(),"application/json");
    let session =
        AiProviderSession::new(settings(base, AiProviderApiProtocol::Anthropic), &req).unwrap();
    complete_chat(&session, &req, &[], &CancellationToken::new())
        .await
        .unwrap();
    let payload = body(&server.join().unwrap());
    let messages = payload["messages"].as_array().unwrap();
    assert_eq!(messages[messages.len() - 2]["content"], blocks);
    assert_eq!(messages.last().unwrap()["content"][0]["is_error"], true);
    assert_eq!(
        messages.last().unwrap()["content"][0]["tool_use_id"],
        "tool-a"
    );
}

#[tokio::test]
async fn responses_replay_ordered_stateless_reasoning_and_call_ids() {
    let output = json!([
        {"type":"reasoning","id":"reason-a","summary":[{"type":"summary_text","text":"check"}],"encrypted_content":"opaque-reason","unknown":42},
        {"type":"function_call","id":"item-a","call_id":"call-a","name":"session_get","arguments":"{\"sessionId\":\"terminal-a\"}","status":"completed"}
    ]);
    let (base, server) = mock_server(json!({"id":"resp-a","object":"response","model":"gpt-test","status":"completed","output":output,"usage":{"input_tokens":2,"output_tokens":3,"total_tokens":5}}).to_string(),"application/json");
    let session = AiProviderSession::new(responses_settings(base), &request()).unwrap();
    let completion = complete_chat(&session, &request(), &[], &CancellationToken::new())
        .await
        .unwrap();
    let payload = body(&server.join().unwrap());
    assert_eq!(payload["store"], false);
    assert!(
        payload["include"]
            .as_array()
            .unwrap()
            .contains(&json!("reasoning.encrypted_content"))
    );
    let req = continuation(request(), completion);
    let (base, server) = mock_server(json!({"id":"resp-b","model":"gpt-test","usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2},"object":"response","status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"ok"}]}]}).to_string(),"application/json");
    let session = AiProviderSession::new(responses_settings(base), &req).unwrap();
    complete_chat(&session, &req, &[], &CancellationToken::new())
        .await
        .unwrap();
    let payload = body(&server.join().unwrap());
    let input = payload["input"].as_array().unwrap();
    assert_eq!(input[input.len() - 3], output[0]);
    assert_eq!(input[input.len() - 2], output[1]);
    assert_eq!(input.last().unwrap()["call_id"], "call-a");
    assert_eq!(input.last().unwrap()["type"], "function_call_output");
}

#[tokio::test]
async fn gemini_replays_function_name_id_and_signature_on_original_part() {
    let parts = json!([{"text":"check","thought":true,"thoughtSignature":"reason-signature"},{"functionCall":{"name":"session_get","args":{"sessionId":"terminal-a"},"id":"gemini-a"},"thoughtSignature":"tool-signature","opaque":7}]);
    let reply = json!({"candidates":[{"index":0,"content":{"role":"model","parts":parts},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":2,"candidatesTokenCount":3,"totalTokenCount":5}});
    let (base, server) = mock_server(reply.to_string(), "application/json");
    let session =
        AiProviderSession::new(settings(base, AiProviderApiProtocol::Gemini), &request()).unwrap();
    let completion = complete_chat(&session, &request(), &[], &CancellationToken::new())
        .await
        .unwrap();
    server.join().unwrap();
    let req = continuation(request(), completion);
    let (base, server) = mock_server(json!({"candidates":[{"content":{"role":"model","parts":[{"text":"ok"}]},"finishReason":"STOP"}]}).to_string(),"application/json");
    let session =
        AiProviderSession::new(settings(base, AiProviderApiProtocol::Gemini), &req).unwrap();
    complete_chat(&session, &req, &[], &CancellationToken::new())
        .await
        .unwrap();
    let payload = body(&server.join().unwrap());
    let contents = payload["contents"].as_array().unwrap();
    assert_eq!(contents[contents.len() - 2]["parts"], parts);
    assert_eq!(
        contents.last().unwrap()["parts"][0]["functionResponse"]["id"],
        "gemini-a"
    );
    assert_eq!(
        contents.last().unwrap()["parts"][0]["functionResponse"]["name"],
        "session_get"
    );
}

#[tokio::test]
async fn sdk_synthetic_end_requires_native_terminal_evidence() {
    for protocol in [
        AiProviderApiProtocol::Gemini,
        AiProviderApiProtocol::Ollama,
        AiProviderApiProtocol::OpenaiCompatible,
    ] {
        for terminal in [false, true] {
            let payload = match protocol {
                AiProviderApiProtocol::Gemini => {
                    let mut frame = json!({"candidates":[{"index":0,"content":{"role":"model","parts":[{"text":"你好"}]}}]});
                    if terminal { frame["candidates"][0]["finishReason"] = json!("STOP"); }
                    sse(&[frame],false)
                }
                AiProviderApiProtocol::Ollama => json!({"model":"gpt-test","message":{"role":"assistant","content":"你好"},"done":terminal}).to_string()+"\n",
                _ => {
                    let mut frames = vec![json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":"你好"})];
                    if terminal { frames.push(json!({"type":"response.completed","response":{"id":"r","status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"你好"}]}]}})); }
                    sse(&frames,false)
                }
            };
            let (base, server) = mock_server(
                payload,
                if protocol == AiProviderApiProtocol::Ollama {
                    "application/x-ndjson"
                } else {
                    "text/event-stream"
                },
            );
            let config = if protocol == AiProviderApiProtocol::OpenaiCompatible {
                responses_settings(base)
            } else {
                settings(base, protocol)
            };
            let (result, deltas) = streamed(config, request()).await;
            server.join().unwrap();
            if terminal {
                assert_eq!(result.unwrap().text, "你好");
                assert_eq!(deltas.iter().filter(|delta| delta.done).count(), 1);
            } else {
                assert_eq!(
                    result.unwrap_err().kind,
                    AiProviderErrorKind::StreamInterrupted
                );
            }
        }
    }
}

#[tokio::test]
async fn anthropic_stream_keeps_multiple_signed_blocks_and_usage() {
    let frames = sse(
        &[
            json!({"type":"message_start","message":{"id":"m","role":"assistant","type":"message","content":[],"usage":{"input_tokens":4,"output_tokens":0}}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"one","signature":"s"}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"1"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"content_block_start","index":1,"content_block":{"type":"redacted_thinking","data":"encrypted"}}),
            json!({"type":"content_block_stop","index":1}),
            json!({"type":"content_block_start","index":2,"content_block":{"type":"thinking","thinking":"two","signature":"s2"}}),
            json!({"type":"content_block_stop","index":2}),
            json!({"type":"content_block_start","index":3,"content_block":{"type":"tool_use","id":"a","name":"session_get","input":{}}}),
            json!({"type":"content_block_delta","index":3,"delta":{"type":"input_json_delta","partial_json":"{}"}}),
            json!({"type":"content_block_stop","index":3}),
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":9}}),
            json!({"type":"message_stop"}),
        ],
        false,
    );
    let (base, server) = mock_server(frames, "text/event-stream");
    let (result, deltas) =
        streamed(settings(base, AiProviderApiProtocol::Anthropic), request()).await;
    server.join().unwrap();
    let result = result.unwrap();
    let blocks = &result.metadata.assistant_turn.provider_items;
    assert_eq!(blocks[0]["signature"], "s1");
    assert_eq!(blocks[1]["data"], "encrypted");
    assert_eq!(blocks[2]["signature"], "s2");
    assert_eq!(blocks[3]["input"], json!({}));
    assert_eq!(result.metadata.usage.output_tokens, Some(9));
    assert_eq!(deltas.iter().filter(|delta| delta.done).count(), 1);
}

#[tokio::test]
async fn complete_object_streams_emit_each_tool_once_before_harness_validation() {
    for protocol in [AiProviderApiProtocol::Gemini, AiProviderApiProtocol::Ollama] {
        let payload = match protocol {
            AiProviderApiProtocol::Gemini => sse(&[json!({"candidates":[{"content":{"role":"model","parts":[
                {"text":"check"},
                {"functionCall":{"name":"session_get","args":{"sessionId":"a"}}},
                {"functionCall":{"name":"session_get","args":{"sessionId":"b"}}}
            ]},"finishReason":"STOP"}]})], false),
            _ => json!({"model":"gpt-test","message":{"role":"assistant","content":"check","tool_calls":[
                {"function":{"name":"session_get","arguments":{"sessionId":"a"}}},
                {"function":{"name":"session_get","arguments":{"sessionId":"b"}}}
            ]},"done":true}).to_string() + "\n",
        };
        let (base, server) = mock_server(
            payload,
            if protocol == AiProviderApiProtocol::Gemini {
                "text/event-stream"
            } else {
                "application/x-ndjson"
            },
        );
        let (result, deltas) = streamed(settings(base, protocol), request()).await;
        server.join().unwrap();
        let result = result.unwrap();
        assert_eq!(result.tool_calls.len(), 2);
        assert_ne!(result.tool_calls[0].id, result.tool_calls[1].id);
        let calls: Vec<_> = deltas
            .iter()
            .flat_map(|delta| &delta.tool_call_deltas)
            .collect();
        assert_eq!(calls.len(), 2);
        for (call, emitted) in result.tool_calls.iter().zip(calls) {
            assert_eq!(call.id, emitted.id_delta);
            assert_eq!(call.arguments.to_string(), emitted.arguments_delta);
        }
        assert_eq!(deltas.iter().filter(|delta| delta.done).count(), 1);
        assert!(
            nyaterm_core::ai::harness::AgentToolRegistry::parse(&result.tool_calls, &result.text)
                .is_err()
        );
    }
}

#[tokio::test]
async fn anthropic_cannot_finish_with_unclosed_tool_arguments() {
    let payload = sse(
        &[
            json!({"type":"message_start","message":{"id":"m","role":"assistant","type":"message","content":[],"usage":{"input_tokens":1,"output_tokens":0}}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"a","name":"session_get","input":{}}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"sessionId\":"}}),
            json!({"type":"message_stop"}),
        ],
        false,
    );
    let (base, server) = mock_server(payload, "text/event-stream");
    let (result, deltas) =
        streamed(settings(base, AiProviderApiProtocol::Anthropic), request()).await;
    server.join().unwrap();
    assert!(result.is_err());
    assert!(!deltas.iter().any(|delta| delta.done));
}

#[tokio::test]
async fn all_compatible_provider_kinds_keep_openai_protocol() {
    for kind in [
        AiProviderKind::Openai,
        AiProviderKind::Deepseek,
        AiProviderKind::Groq,
        AiProviderKind::Xai,
        AiProviderKind::Cohere,
        AiProviderKind::Mimo,
        AiProviderKind::Zai,
        AiProviderKind::OpenaiCompatible,
        AiProviderKind::Ollama,
    ] {
        let (base,server) = mock_server(json!({"choices":[{"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}]}).to_string(),"application/json");
        let mut config = settings(base, AiProviderApiProtocol::OpenaiCompatible);
        config.models[0].provider_kind = Some(kind.clone());
        config.provider_credentials[0].provider_kind = kind.clone();
        config.models[0].name = "deepseek-v4-flash-none".into();
        let session = AiProviderSession::new(config, &request()).unwrap();
        complete_chat(&session, &request(), &[], &CancellationToken::new())
            .await
            .unwrap();
        let raw = server.join().unwrap();
        assert!(raw.starts_with("POST /v1/chat/completions "));
        assert_eq!(
            body(&raw)["model"],
            if kind == AiProviderKind::Deepseek {
                "deepseek-v4-flash"
            } else {
                "deepseek-v4-flash-none"
            }
        );
    }
}

#[tokio::test]
async fn anonymous_discovery_keeps_query_and_model_identity() {
    let (base, server) = mock_server(
        json!({"data":[{"id":"model-b"},{"id":"model-a"},{"id":"model-b"}]}).to_string(),
        "application/json",
    );
    let mut config = settings(
        format!("{base}?tenant=one"),
        AiProviderApiProtocol::OpenaiCompatible,
    );
    config.provider_credentials[0].api_key = None;
    let models = super::discover_provider_models(
        &config,
        &config.provider_credentials[0],
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(models.len(), 2);
    assert_eq!(models[0].name, "model-a");
    assert_eq!(models[0].credential_id.as_deref(), Some("mock-responses"));
    let raw = server.join().unwrap();
    assert!(raw.starts_with("GET /v1/models?tenant=one "));
    assert!(!raw.to_lowercase().contains("authorization:"));
}

#[tokio::test]
async fn old_account_snapshot_and_new_account_are_isolated() {
    let response =
        json!({"choices":[{"message":{"content":"ok"},"finish_reason":"stop"}]}).to_string();
    let (old_url, old_server) = mock_server(response.clone(), "application/json");
    let (new_url, new_server) = mock_server(response, "application/json");
    let mut config = settings(old_url, AiProviderApiProtocol::OpenaiCompatible);
    config.provider_credentials[0].api_key = Some("old-fixture".into());
    let old = AiProviderSession::new(config.clone(), &request()).unwrap();
    config.provider_credentials[0].api_key = Some("new-fixture".into());
    config.provider_credentials[0].base_url = Some(new_url);
    let new = AiProviderSession::new(config, &request()).unwrap();
    complete_chat(&old, &request(), &[], &CancellationToken::new())
        .await
        .unwrap();
    complete_chat(&new, &request(), &[], &CancellationToken::new())
        .await
        .unwrap();
    let old_wire = old_server.join().unwrap();
    let new_wire = new_server.join().unwrap();
    assert!(old_wire.contains("old-fixture") && !old_wire.contains("new-fixture"));
    assert!(new_wire.contains("new-fixture") && !new_wire.contains("old-fixture"));
}

#[tokio::test]
async fn ollama_compatibility_backend_preserves_literal_model_and_options() {
    let (base,server) = mock_server(json!({"model":"org::model","message":{"role":"assistant","content":"ok"},"done":true,"eval_count":3,"prompt_eval_count":2}).to_string(),"application/json");
    let mut config = settings(base, AiProviderApiProtocol::Ollama);
    config.models[0].name = "org::model".into();
    let mut req = request();
    req.options.generation.extra_body = Some(json!({"keep_alive":"5m","options":{"num_ctx":8192}}));
    let session = AiProviderSession::new(config, &req).unwrap();
    assert!(session.ollama_compat);
    let result = complete_chat(&session, &req, &[], &CancellationToken::new())
        .await
        .unwrap();
    let payload = body(&server.join().unwrap());
    assert_eq!(payload["model"], "org::model");
    assert_eq!(payload["keep_alive"], "5m");
    assert_eq!(payload["options"]["num_ctx"], 8192);
    assert_eq!(result.metadata.usage.total_tokens, Some(5));
}

#[tokio::test]
async fn ollama_compatibility_classifies_completed_replies_in_both_modes() {
    use nyaterm_core::ai::harness::{AgentDecision, AgentResponseError, AgentToolRegistry};

    for streaming in [false, true] {
        for (message, expected) in [
            (json!({"content":""}), Some(AgentResponseError::Empty)),
            (
                json!({"content":"","thinking":"private-reasoning"}),
                Some(AgentResponseError::ReasoningOnly),
            ),
            (
                json!({"tool_calls":[{"id":"bad-args","function":{"name":"final_answer","arguments":[]}}]}),
                None,
            ),
        ] {
            let raw =
                json!({"model":"org::model","message":message,"done":true,"done_reason":"stop"});
            let (base, server) = mock_server(
                format!("{raw}\n"),
                if streaming {
                    "application/x-ndjson"
                } else {
                    "application/json"
                },
            );
            let mut config = settings(base, AiProviderApiProtocol::Ollama);
            config.models[0].name = "org::model".into();
            let mut req = request();
            req.mode = AiMode::Agent;
            let completion = if streaming {
                streamed(config, req).await.0.unwrap()
            } else {
                let session = AiProviderSession::new(config, &req).unwrap();
                complete_chat(&session, &req, &[], &CancellationToken::new())
                    .await
                    .unwrap()
            };
            server.join().unwrap();
            let decision = AgentToolRegistry::decision(&completion);
            if let Some(expected) = expected {
                assert_eq!(decision.unwrap_err(), expected);
            } else {
                let AgentDecision::Tools(batch) = decision.unwrap() else {
                    panic!("invalid arguments must reach the Agent tool feedback path");
                };
                assert_eq!(batch.calls[0].source.id.as_deref(), Some("bad-args"));
                assert_eq!(
                    batch.calls[0].action,
                    Err(AgentResponseError::InvalidArguments)
                );
            }
        }
    }
}

#[tokio::test]
async fn connectivity_tests_keep_64_token_limit() {
    let (base, server) = mock_server(
        json!({"choices":[{"message":{"content":"ok"},"finish_reason":"stop"}]}).to_string(),
        "application/json",
    );
    let config = settings(base, AiProviderApiProtocol::OpenaiCompatible);
    super::test_model_connection(
        &config,
        "mock-responses:gpt-test",
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    let payload = body(&server.join().unwrap());
    assert_eq!(payload["max_tokens"], 64);
    assert!(payload.get("tools").is_none());
    assert!(payload.get("reasoning_effort").is_none());
}

fn mock_sequence(responses: Vec<String>) -> (String, thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let mut requests = Vec::new();
        for response in responses {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            requests.push(read_http_request(&mut stream));
            stream
                .write_all(
                    response
                        .replace("{ORIGIN}", &format!("http://{address}"))
                        .as_bytes(),
                )
                .unwrap();
        }
        requests
    });
    (format!("http://{address}/v1"), handle)
}
fn http_json(value: Value) -> String {
    let body = value.to_string();
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

#[tokio::test]
async fn multiple_tools_continue_as_one_assistant_turn_on_all_five_protocols() {
    use nyaterm_core::ai::harness::{AgentDecision, AgentRequestContext, AgentToolRegistry};
    for (protocol, responses) in [
        (AiProviderApiProtocol::OpenaiCompatible, false),
        (AiProviderApiProtocol::OpenaiCompatible, true),
        (AiProviderApiProtocol::Anthropic, false),
        (AiProviderApiProtocol::Gemini, false),
        (AiProviderApiProtocol::Ollama, false),
    ] {
        let (first, second) = if responses {
            (
                json!({"id":"resp-a","object":"response","model":"gpt-test","status":"completed","output":[
                    {"type":"reasoning","id":"reason-a","summary":[],"encrypted_content":"opaque"},
                    {"type":"function_call","id":"item-a","call_id":"a","name":"session_get","arguments":"{\"sessionId\":\"s\"}"},
                    {"type":"function_call","id":"item-b","call_id":"b","name":"session_get","arguments":"{\"sessionId\":\"s\"}"}
                ]}),
                json!({"id":"resp-b","object":"response","model":"gpt-test","status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Done"}]}]}),
            )
        } else {
            match protocol {
                AiProviderApiProtocol::OpenaiCompatible => (
                    json!({"choices":[{"message":{"role":"assistant","content":"Inspecting","tool_calls":[
                    {"id":"a","type":"function","function":{"name":"session_get","arguments":"{\"sessionId\":\"s\"}"}},
                    {"id":"b","type":"function","function":{"name":"session_get","arguments":"{\"sessionId\":\"s\"}"}}
                ]},"finish_reason":"tool_calls"}]}),
                    json!({"choices":[{"message":{"role":"assistant","content":"Done"},"finish_reason":"stop"}]}),
                ),
                AiProviderApiProtocol::Anthropic => (
                    json!({"id":"msg-a","type":"message","role":"assistant","content":[
                    {"type":"thinking","thinking":"Inspecting","signature":"signed"},
                    {"type":"tool_use","id":"a","name":"session_get","input":{"sessionId":"s"}},
                    {"type":"tool_use","id":"b","name":"session_get","input":{"sessionId":"s"}}
                ],"stop_reason":"tool_use","usage":{"input_tokens":1,"output_tokens":1}}),
                    json!({"id":"msg-b","type":"message","role":"assistant","content":[{"type":"text","text":"Done"}],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}}),
                ),
                AiProviderApiProtocol::Gemini => (
                    json!({"candidates":[{"content":{"role":"model","parts":[
                    {"text":"Inspecting","thought":true,"thoughtSignature":"signed"},
                    {"functionCall":{"id":"a","name":"session_get","args":{"sessionId":"s"}},"thoughtSignature":"tool-a"},
                    {"functionCall":{"id":"b","name":"session_get","args":{"sessionId":"s"}},"thoughtSignature":"tool-b"}
                ]},"finishReason":"STOP"}]}),
                    json!({"candidates":[{"content":{"role":"model","parts":[{"text":"Done"}]},"finishReason":"STOP"}]}),
                ),
                AiProviderApiProtocol::Ollama => (
                    json!({"message":{"role":"assistant","content":"Inspecting","tool_calls":[
                    {"function":{"name":"session_get","arguments":{"sessionId":"s"}}},
                    {"function":{"name":"session_get","arguments":{"sessionId":"s"}}}
                ]},"done":true,"done_reason":"stop"}),
                    json!({"message":{"role":"assistant","content":"Done"},"done":true,"done_reason":"stop"}),
                ),
            }
        };
        let (base, server) = mock_sequence(vec![http_json(first.clone()), http_json(second)]);
        let config = if responses {
            responses_settings(base)
        } else {
            settings(base, protocol)
        };
        let mut req = request();
        req.mode = AiMode::Agent;
        req.options.agent_context = Some(AgentRequestContext {
            run_id: "run".into(),
            turns: vec![],
            remaining_steps: 4,
        });
        let session = AiProviderSession::new(config, &req).unwrap();
        let completion = complete_chat(&session, &req, &[], &CancellationToken::new())
            .await
            .unwrap_or_else(|error| panic!("{protocol:?} responses={responses}: {error:?}"));
        assert_eq!(
            completion.tool_calls.len(),
            2,
            "{protocol:?} responses={responses}"
        );
        let req = continuation(req, completion);
        let completion = complete_chat(&session, &req, &[], &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(
            AgentToolRegistry::decision(&completion).unwrap(),
            AgentDecision::FinalText("Done".into())
        );
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 2);
        let payload = body(&requests[1]);
        if responses {
            let input = payload["input"].as_array().unwrap();
            assert_eq!(
                input
                    .iter()
                    .filter(|item| item["type"] == "reasoning")
                    .count(),
                1
            );
            assert_eq!(
                input
                    .iter()
                    .filter(|item| item["type"] == "function_call")
                    .count(),
                2
            );
            assert_eq!(
                input
                    .iter()
                    .filter(|item| item["type"] == "function_call_output")
                    .count(),
                2
            );
            assert_eq!(payload["tools"][0]["strict"], false);
        } else if protocol == AiProviderApiProtocol::Gemini {
            let contents = payload["contents"].as_array().unwrap();
            assert_eq!(
                contents[contents.len() - 2]["parts"],
                first["candidates"][0]["content"]["parts"]
            );
            assert_eq!(
                contents.last().unwrap()["parts"][0]["functionResponse"]["id"],
                "a"
            );
            assert_eq!(
                contents.last().unwrap()["parts"][1]["functionResponse"]["id"],
                "b"
            );
        } else {
            let messages = payload["messages"].as_array().unwrap();
            assert_eq!(
                messages
                    .iter()
                    .filter(|message| message["role"] == "assistant")
                    .count(),
                1
            );
            if protocol == AiProviderApiProtocol::Anthropic {
                assert_eq!(messages[messages.len() - 2]["content"], first["content"]);
                assert_eq!(
                    messages.last().unwrap()["content"]
                        .as_array()
                        .unwrap()
                        .len(),
                    2
                );
            } else {
                let assistant = messages
                    .iter()
                    .find(|message| message["role"] == "assistant")
                    .unwrap();
                assert_eq!(assistant["tool_calls"].as_array().unwrap().len(), 2);
                assert_eq!(
                    messages
                        .iter()
                        .filter(|message| message["role"] == "tool")
                        .count(),
                    2
                );
            }
        }
    }
}

#[test]
fn provider_diagnostics_allow_only_known_codes_parameters_and_safe_request_ids() {
    use nyaterm_core::ai::completion::{AiProviderDiagnosticCode, AiProviderParameter};
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("x-request-id", "req_fixture_123".parse().unwrap());
    let error = super::error::status_error(
        400,
        &headers,
        Some(
            &json!({"error":{"code":"unsupported_parameter","param":"reasoning_effort","message":"private-user-data"}}),
        ),
    );
    assert_eq!(
        error.diagnostic_code,
        Some(AiProviderDiagnosticCode::UnsupportedParameter)
    );
    assert_eq!(error.parameter, Some(AiProviderParameter::Reasoning));
    assert_eq!(error.request_id.as_deref(), Some("req_fixture_123"));
    assert!(!error.tool_protocol_unsupported);
    assert!(!format!("{error:?} {error}").contains("private"));
    headers.insert("x-request-id", "private-user-data".parse().unwrap());
    let error = super::error::status_error(
        400,
        &headers,
        Some(
            &json!({"error":{"code":"private-code","param":"private-param","message":"private-message"}}),
        ),
    );
    assert!(
        error.diagnostic_code.is_none() && error.parameter.is_none() && error.request_id.is_none()
    );
    assert!(!format!("{error:?} {error}").contains("private"));
}

#[tokio::test]
async fn discovery_paginates_and_rejects_repeated_cursor() {
    for repeated in [false, true] {
        let first = json!({"models":[{"name":"models/a","supportedGenerationMethods":["generateContent"]}],"nextPageToken":"next"});
        let mut second = json!({"models":[{"name":"models/b","supportedGenerationMethods":["generateContent"]}]});
        if repeated {
            second["nextPageToken"] = json!("next");
        }
        let (base, server) = mock_sequence(vec![http_json(first), http_json(second)]);
        let config = settings(base, AiProviderApiProtocol::Gemini);
        let result = super::discover_provider_models(
            &config,
            &config.provider_credentials[0],
            &CancellationToken::new(),
        )
        .await;
        let requests = server.join().unwrap();
        assert!(requests[1].contains("pageToken=next"));
        if repeated {
            assert_eq!(result.unwrap_err().kind, AiProviderErrorKind::Protocol);
        } else {
            assert_eq!(result.unwrap().len(), 2);
        }
    }
}

#[tokio::test]
async fn redirect_policy_keeps_custom_credentials_on_their_origin() {
    let reply = http_json(json!({"choices":[{"message":{"content":"ok"},"finish_reason":"stop"}]}));
    let (base,server) = mock_sequence(vec!["HTTP/1.1 307 Temporary Redirect\r\nLocation: {ORIGIN}/next\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into(),reply]);
    let mut config = settings(base, AiProviderApiProtocol::OpenaiCompatible);
    config.provider_credentials[0].extra_headers = vec![nyaterm_core::AiHttpHeader {
        id: "h".into(),
        name: "x-gateway-secret".into(),
        value: "fixture-secret".into(),
    }];
    let session = AiProviderSession::new(config.clone(), &request()).unwrap();
    complete_chat(&session, &request(), &[], &CancellationToken::new())
        .await
        .unwrap();
    let requests = server.join().unwrap();
    assert!(requests[1].contains("x-gateway-secret: fixture-secret"));
    let victim = TcpListener::bind("127.0.0.1:0").unwrap();
    victim.set_nonblocking(true).unwrap();
    let (base, server) = mock_sequence(vec![format!(
        "HTTP/1.1 307 Temporary Redirect\r\nLocation: http://{}/leak?secret=query\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        victim.local_addr().unwrap()
    )]);
    config.provider_credentials[0].base_url = Some(base);
    let session = AiProviderSession::new(config, &request()).unwrap();
    let error = complete_chat(&session, &request(), &[], &CancellationToken::new())
        .await
        .unwrap_err();
    server.join().unwrap();
    assert!(!error.to_string().contains("query"));
    assert!(matches!(victim.accept(),Err(error) if error.kind()==std::io::ErrorKind::WouldBlock));
}

#[tokio::test]
async fn sdk_preserves_legacy_openai_business_request_and_result() {
    let reply = json!({"choices":[{"message":{"content":"answer","reasoning_content":"reason","tool_calls":[{"id":"a","type":"function","function":{"name":"final_answer","arguments":"{\"answer\":\"done\"}"}}]},"finish_reason":"tool_calls"}]}).to_string();
    let (base, server) = mock_server(reply.clone(), "application/json");
    let config = settings(base, AiProviderApiProtocol::OpenaiCompatible);
    let req = request();
    // The pure compatibility contracts remain useful to backups/parity tests;
    // the old blocking HTTP/SSE backend is no longer needed as a reference.
    let model = nyaterm_core::resolve_request_model(&config, &req).unwrap();
    let old_body =
        nyaterm_core::build_openai_compatible_chat_request_body(&model, &req, &config, &[]);
    let legacy = nyaterm_core::parse_openai_compatible_chat_response(&reply).unwrap();
    let session = AiProviderSession::new(config, &request()).unwrap();
    let sdk = complete_chat(&session, &request(), &[], &CancellationToken::new())
        .await
        .unwrap();
    let new_body = body(&server.join().unwrap());
    assert_eq!(old_body["model"], new_body["model"]);
    assert_eq!(old_body["messages"], new_body["messages"]);
    assert_eq!(legacy.text, sdk.text);
    assert_eq!(legacy.reasoning_content, sdk.reasoning_content);
    assert_eq!(legacy.tool_calls, sdk.tool_calls);
}

#[test]
fn invalid_header_value_fails_safely_before_connecting() {
    let mut config = settings(
        "http://127.0.0.1:1/v1".into(),
        AiProviderApiProtocol::OpenaiCompatible,
    );
    config.provider_credentials[0].extra_headers = vec![nyaterm_core::AiHttpHeader {
        id: "h".into(),
        name: "x-test".into(),
        value: "秘密".into(),
    }];
    let session = AiProviderSession::new(config, &request()).unwrap();
    let error = session
        .target(false)
        .expect_err("SDK header values must be ASCII");
    assert_eq!(error.kind, AiProviderErrorKind::InvalidRequest);
    assert!(!format!("{error:?}").contains("秘密"));
}

#[test]
fn server_or_authentication_errors_never_allow_tool_protocol_fallback() {
    let headers = reqwest::header::HeaderMap::new();
    let body = json!({"error":{"code":"unsupported_tools"}});
    for status in [401, 403, 429, 500, 503] {
        assert!(
            !super::error::status_error(status, &headers, Some(&body)).tool_protocol_unsupported
        );
    }
    assert!(super::error::status_error(400, &headers, Some(&body)).tool_protocol_unsupported);
}

#[tokio::test]
async fn reasoning_none_and_ultra_keep_their_explicit_wire_values() {
    for (effort, wire) in [
        (AiReasoningEffort::None, "none"),
        (AiReasoningEffort::Ultra, "ultra"),
    ] {
        let (base, server) = mock_server(
            json!({"choices":[{"message":{"content":"ok"},"finish_reason":"stop"}]}).to_string(),
            "application/json",
        );
        let mut config = settings(base, AiProviderApiProtocol::OpenaiCompatible);
        config.default_reasoning_effort = effort;
        let session = AiProviderSession::new(config, &request()).unwrap();
        complete_chat(&session, &request(), &[], &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(body(&server.join().unwrap())["reasoning_effort"], wire);
    }
}

#[test]
fn explicit_anthropic_budget_requires_output_room_and_auto_tool_choice() {
    let config = settings(
        "http://127.0.0.1:1/v1".into(),
        AiProviderApiProtocol::Anthropic,
    );
    let mut req = request();
    req.mode = AiMode::Agent;
    req.options.generation.thinking_budget = Some(4096);
    req.options.generation.max_tokens = Some(4096);
    let session = AiProviderSession::new(config, &req).unwrap();
    assert_eq!(
        super::request::convert(&session, &req, &[])
            .unwrap_err()
            .kind,
        AiProviderErrorKind::InvalidRequest
    );
    req.options.generation.max_tokens = Some(8192);
    let (_, options) = super::request::convert(&session, &req, &[]).unwrap();
    assert!(matches!(
        options.tool_choice,
        Some(genai::chat::ToolChoice::Auto)
    ));
    assert_eq!(options.max_tokens, Some(8192));
}

#[test]
fn anthropic_default_thinking_families_keep_compatible_tool_choice() {
    for (model, auto) in [
        ("claude-sonnet-4-6", true),
        ("claude-opus-5", true),
        ("claude-haiku-6", true),
        ("claude-opus-50", true),
    ] {
        let mut config = settings(
            "http://127.0.0.1:1/v1".into(),
            AiProviderApiProtocol::Anthropic,
        );
        config.models[0].name = model.into();
        let mut req = request();
        req.mode = AiMode::Agent;
        let session = AiProviderSession::new(config, &req).unwrap();
        let (_, options) = super::request::convert(&session, &req, &[]).unwrap();
        assert_eq!(
            matches!(options.tool_choice, Some(genai::chat::ToolChoice::Auto)),
            auto,
            "{model}"
        );
        assert!(options.reasoning_effort.is_none());
    }
}

#[tokio::test]
async fn discovery_keeps_empty_invalid_and_pre_cancelled_contracts() {
    for (reply, succeeds) in [(r#"{"data":[]}"#, true), ("{}", false), ("not-json", false)] {
        let (base, server) = mock_server(reply.into(), "application/json");
        let config = settings(base, AiProviderApiProtocol::OpenaiCompatible);
        let result = super::discover_provider_models(
            &config,
            &config.provider_credentials[0],
            &CancellationToken::new(),
        )
        .await;
        server.join().unwrap();
        assert_eq!(result.is_ok(), succeeds);
        if let Ok(models) = result {
            assert!(models.is_empty());
        }
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let config = settings(
        format!("http://{}/v1", listener.local_addr().unwrap()),
        AiProviderApiProtocol::OpenaiCompatible,
    );
    let cancel = CancellationToken::new();
    cancel.cancel();
    let error = super::discover_provider_models(&config, &config.provider_credentials[0], &cancel)
        .await
        .unwrap_err();
    assert_eq!(error.kind, AiProviderErrorKind::Cancelled);
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[tokio::test]
async fn timeout_interrupts_provider_response_wait_without_retry() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut config = settings(
        format!("http://{}/v1", listener.local_addr().unwrap()),
        AiProviderApiProtocol::OpenaiCompatible,
    );
    config.timeout_ms = 100;
    let session = AiProviderSession::new(config, &request()).unwrap();
    let error = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        complete_chat(&session, &request(), &[], &CancellationToken::new()),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert_eq!(error.kind, AiProviderErrorKind::Timeout);
}
