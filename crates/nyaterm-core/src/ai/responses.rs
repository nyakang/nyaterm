//! OpenAI Responses API wire format.
//!
//! This module is intentionally transport-free. The desktop adapter owns HTTP,
//! timeouts and cancellation; core owns endpoint joining, request JSON and
//! response/SSE interpretation so it can be tested with fixtures.

use super::agent::agent_openai_tools;
use super::{
    AiApiFormat, AiChatCompletion, AiChatRequest, AiChatStreamDelta, AiMessage, AiModelError,
    AiProviderKind, AiReasoningEffort, AiSettings, AiToolCall, AiToolCallDelta, ResolvedAiModel,
    build_openai_compatible_chat_request_body_with_stream, trim_string_to_option,
};

const OPENAI_DEFAULT_BASE_URL: &str = "https://api.openai.com/v1/";

pub fn uses_responses_api(model: &ResolvedAiModel) -> bool {
    model.api_format == AiApiFormat::Responses
        && model.credential.as_ref().is_none_or(|credential| {
            super::provider_settings::effective_protocol(credential)
                == super::AiProviderApiProtocol::OpenaiCompatible
        })
}

pub fn openai_responses_url(model: &ResolvedAiModel) -> Result<String, AiModelError> {
    let configured = model
        .credential
        .as_ref()
        .and_then(|credential| credential.base_url.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let base_url = match (configured, &model.provider_kind) {
        (Some(base_url), _) => base_url,
        (None, AiProviderKind::Openai) => OPENAI_DEFAULT_BASE_URL,
        (None, AiProviderKind::OpenaiCompatible) => {
            return Err(AiModelError::InvalidBaseUrl {
                base_url: String::new(),
                message: "Responses API requires a base URL for OpenAI-compatible credentials"
                    .to_string(),
            });
        }
        (None, provider_kind) => {
            return Err(AiModelError::InvalidBaseUrl {
                base_url: String::new(),
                message: format!("Responses API is not supported for {provider_kind:?}"),
            });
        }
    };
    super::providers::join_api_base_url(base_url, "responses")
}

pub fn build_openai_responses_request_body(
    resolved_model: &ResolvedAiModel,
    request: &AiChatRequest,
    settings: &AiSettings,
    history: &[AiMessage],
    stream: bool,
) -> serde_json::Value {
    // Reuse the canonical prompt/history builder, then translate only the
    // provider envelope. This keeps Chat Completions and Responses context
    // selection byte-for-byte aligned.
    let mut base_request = request.clone();
    if let Some(context) = &mut base_request.options.agent_context {
        context.turns.clear();
    }
    let chat = build_openai_compatible_chat_request_body_with_stream(
        resolved_model,
        &base_request,
        settings,
        history,
        stream,
    );
    let mut body = serde_json::json!({
        "model": resolved_model.model_name,
        "input": super::harness::transcript::responses_input(chat.get("messages").and_then(serde_json::Value::as_array).map(Vec::as_slice).unwrap_or_default()),
        "stream": stream,
        "store": false,
    });
    if let Some(context) = &request.options.agent_context {
        let input = body["input"].as_array_mut().expect("responses input");
        if request.options.agent_json_protocol {
            let mut messages = Vec::new();
            super::harness::transcript::append_openai_with_language(
                &mut messages,
                context,
                true,
                &request.options.language,
            );
            input.extend(super::harness::transcript::responses_input(&messages));
        } else {
            super::harness::transcript::append_responses(input, context);
        }
    }
    if let Some(effort) = responses_reasoning_effort(&settings.default_reasoning_effort) {
        body["reasoning"] = serde_json::json!({ "effort": effort });
    }
    if super::harness::AgentToolRegistry::request_policy(
        request,
        super::AiProviderApiProtocol::OpenaiCompatible,
    )
    .tools
    .is_some()
    {
        body["tools"] = responses_tools(request);
        body["tool_choice"] = serde_json::json!("auto");
    }
    if request.options.connectivity_test {
        body["max_output_tokens"] = serde_json::json!(64);
    }
    body
}

pub fn parse_openai_responses_response(body: &str) -> Result<AiChatCompletion, AiModelError> {
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|error| AiModelError::InvalidChatJson(error.to_string()))?;
    if let Some(error) = response_error_message(&value) {
        return Err(AiModelError::ResponsesError(error));
    }
    let text = completed_output_text(&value).unwrap_or_default();
    let reasoning_content = completed_reasoning_text(&value);
    let tool_calls = completed_tool_calls(&value)?;
    if text.is_empty() && reasoning_content.is_none() && tool_calls.is_empty() {
        return Err(AiModelError::MissingChatContent);
    }
    Ok(AiChatCompletion {
        metadata: Default::default(),
        text,
        reasoning_content,
        tool_calls,
    })
}

pub fn parse_openai_responses_stream_chunk(
    chunk: &str,
) -> Result<Vec<AiChatStreamDelta>, AiModelError> {
    let mut deltas = Vec::new();
    for block in split_sse_blocks(chunk) {
        let Some(value) = parse_sse_value(&block)? else {
            continue;
        };
        let kind = value
            .get("type")
            .and_then(serde_json::Value::as_str)
            .or_else(|| sse_event_type(&block))
            .unwrap_or_default();
        if kind == "error" || kind == "response.failed" {
            return Err(AiModelError::ResponsesError(
                response_error_message(&value)
                    .unwrap_or_else(|| "unknown Responses API stream error".to_string()),
            ));
        }
        match kind {
            "response.output_text.delta" => deltas.push(AiChatStreamDelta {
                text_delta: value
                    .get("delta")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                ..Default::default()
            }),
            "response.function_call_arguments.delta" => {
                deltas.push(AiChatStreamDelta {
                    tool_call_deltas: vec![AiToolCallDelta {
                        thought_signature: None,
                        index: response_output_index(&value),
                        id_delta: value
                            .get("item_id")
                            .or_else(|| value.get("call_id"))
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string),
                        name_delta: None,
                        arguments_delta: value
                            .get("delta")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    }],
                    ..Default::default()
                });
            }
            "response.output_item.added" => {
                if let Some(item) = value.get("item")
                    && item.get("type").and_then(serde_json::Value::as_str) == Some("function_call")
                {
                    deltas.push(AiChatStreamDelta {
                        tool_call_deltas: vec![AiToolCallDelta {
                            thought_signature: None,
                            index: response_output_index(&value),
                            id_delta: item
                                .get("call_id")
                                .or_else(|| item.get("id"))
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_string),
                            name_delta: item
                                .get("name")
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_string),
                            arguments_delta: String::new(),
                        }],
                        ..Default::default()
                    });
                }
            }
            "response.completed" => {
                deltas.push(AiChatStreamDelta {
                    done: true,
                    ..Default::default()
                });
            }
            _ if kind.ends_with(".delta") && kind.contains("reasoning") => {
                deltas.push(AiChatStreamDelta {
                    reasoning_delta: Some(
                        value
                            .get("delta")
                            .or_else(|| value.get("text"))
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    ),
                    ..Default::default()
                });
            }
            _ => {}
        }
    }
    Ok(deltas)
}

pub fn responses_reasoning_effort(value: &AiReasoningEffort) -> Option<&'static str> {
    match value {
        AiReasoningEffort::Auto => None,
        AiReasoningEffort::None => Some("none"),
        AiReasoningEffort::Minimal => Some("minimal"),
        AiReasoningEffort::Max => Some("max"),
        AiReasoningEffort::Ultra => Some("ultra"),
        AiReasoningEffort::Low => Some("low"),
        AiReasoningEffort::Medium => Some("medium"),
        AiReasoningEffort::High => Some("high"),
        AiReasoningEffort::XHigh => Some("xhigh"),
    }
}

fn responses_tools(request: &AiChatRequest) -> serde_json::Value {
    let tools = agent_openai_tools(super::harness::AgentToolRegistry::available_tools(request));
    serde_json::Value::Array(
        tools
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|tool| tool.get("function"))
            .map(|function| {
                serde_json::json!({
                    "type": "function",
                    "name": function.get("name").cloned().unwrap_or_default(),
                    "description": function.get("description").cloned().unwrap_or_default(),
                    "parameters": function.get("parameters").cloned().unwrap_or_else(|| serde_json::json!({})),
                    "strict": false,
                })
            })
            .collect(),
    )
}

fn split_sse_blocks(chunk: &str) -> Vec<String> {
    chunk
        .replace("\r\n", "\n")
        .split("\n\n")
        .map(str::trim)
        .filter(|block| !block.is_empty())
        .map(str::to_string)
        .collect()
}

fn parse_sse_value(block: &str) -> Result<Option<serde_json::Value>, AiModelError> {
    let data = block
        .lines()
        .filter_map(|line| line.strip_prefix("data:").map(str::trim_start))
        .collect::<Vec<_>>()
        .join("\n");
    if data.is_empty() || data == "[DONE]" {
        return Ok(None);
    }
    serde_json::from_str(&data)
        .map(Some)
        .map_err(|error| AiModelError::InvalidChatJson(error.to_string()))
}

fn sse_event_type(block: &str) -> Option<&str> {
    block
        .lines()
        .find_map(|line| line.strip_prefix("event:").map(str::trim))
}

fn response_output_index(value: &serde_json::Value) -> usize {
    value
        .get("output_index")
        .or_else(|| value.get("item_index"))
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or_default()
}

fn response_error_message(value: &serde_json::Value) -> Option<String> {
    value
        .get("error")
        .and_then(|error| {
            error
                .get("message")
                .or_else(|| error.get("type"))
                .and_then(serde_json::Value::as_str)
        })
        .or_else(|| value.get("message").and_then(serde_json::Value::as_str))
        .map(str::to_string)
}

fn completed_output_text(response: &serde_json::Value) -> Option<String> {
    if let Some(output_text) = response
        .get("output_text")
        .and_then(serde_json::Value::as_str)
    {
        return trim_string_to_option(output_text.to_string());
    }
    let mut text = String::new();
    for item in response.get("output")?.as_array()? {
        if item.get("type").and_then(serde_json::Value::as_str) != Some("message") {
            continue;
        }
        for part in item
            .get("content")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
        {
            if part.get("type").and_then(serde_json::Value::as_str) == Some("output_text")
                && let Some(part_text) = part.get("text").and_then(serde_json::Value::as_str)
            {
                text.push_str(part_text);
            }
        }
    }
    trim_string_to_option(text)
}

fn completed_reasoning_text(response: &serde_json::Value) -> Option<String> {
    let mut text = String::new();
    for item in response.get("output")?.as_array()? {
        if item.get("type").and_then(serde_json::Value::as_str) != Some("reasoning") {
            continue;
        }
        for key in ["summary", "content"] {
            for part in item
                .get(key)
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
            {
                if let Some(part_text) = part
                    .get("text")
                    .or_else(|| part.get("summary"))
                    .and_then(serde_json::Value::as_str)
                {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(part_text);
                }
            }
        }
    }
    trim_string_to_option(text)
}

fn completed_tool_calls(response: &serde_json::Value) -> Result<Vec<AiToolCall>, AiModelError> {
    let mut calls = Vec::new();
    for item in response
        .get("output")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        if item.get("type").and_then(serde_json::Value::as_str) != Some("function_call") {
            continue;
        }
        let Some(name) = item
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
        else {
            continue;
        };
        let raw_arguments = item
            .get("arguments")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("{}");
        let arguments = serde_json::from_str(raw_arguments)
            .map_err(|error| AiModelError::InvalidChatJson(error.to_string()))?;
        calls.push(AiToolCall {
            thought_signature: None,
            id: item
                .get("call_id")
                .or_else(|| item.get("id"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            name: name.to_string(),
            arguments,
        });
    }
    Ok(calls)
}

#[cfg(test)]
mod tests {
    use super::{
        AiApiFormat, AiModelError, AiProviderKind, AiReasoningEffort, AiSettings, ResolvedAiModel,
        build_openai_responses_request_body, openai_responses_url, parse_openai_responses_response,
        parse_openai_responses_stream_chunk,
    };
    use crate::ai::{AiBackendKind, AiMode, AiProviderCredential};

    fn resolved_model(base_url: Option<&str>) -> ResolvedAiModel {
        ResolvedAiModel {
            model_name: "gpt-test".to_string(),
            backend: AiBackendKind::Genai,
            provider_kind: AiProviderKind::OpenaiCompatible,
            api_format: AiApiFormat::Responses,
            credential: Some(AiProviderCredential {
                icon_data_url: None,
                api_protocol: None,
                id: "credential-test".to_string(),
                name: "Test".to_string(),
                provider_kind: AiProviderKind::OpenaiCompatible,
                api_format: AiApiFormat::Responses,
                base_url: base_url.map(str::to_string),
                api_key: Some("fixture-key".to_string().into()),
                extra_headers: Vec::new(),
                enabled: true,
            }),
        }
    }

    #[test]
    fn responses_url_joins_path_and_preserves_query() {
        assert_eq!(
            openai_responses_url(&resolved_model(Some(
                "https://api.example.invalid/v1?api-version=1"
            )))
            .unwrap(),
            "https://api.example.invalid/v1/responses?api-version=1"
        );
    }

    #[test]
    fn response_body_includes_reasoning_and_never_stores_provider_data() {
        let settings = AiSettings {
            default_reasoning_effort: AiReasoningEffort::High,
            ..AiSettings::default()
        };
        let mut request = crate::ai::tests::sample_ai_request("en");
        request.mode = AiMode::Ask;
        let body = build_openai_responses_request_body(
            &resolved_model(Some("https://api.example.invalid/v1")),
            &request,
            &settings,
            &[],
            true,
        );
        assert_eq!(body["reasoning"]["effort"], "high");
        assert_eq!(body["store"], false);
        assert_eq!(body["stream"], true);
        assert!(
            body["input"]
                .as_array()
                .is_some_and(|input| input.len() == 2)
        );
    }

    #[test]
    fn parses_text_reasoning_completion_and_function_calls() {
        let parsed = parse_openai_responses_response(
            r#"{"output":[{"type":"reasoning","summary":[{"type":"summary_text","text":"inspect"}]},{"type":"message","content":[{"type":"output_text","text":"done"}]},{"type":"function_call","call_id":"call-1","name":"execute_command","arguments":"{\"command\":\"df -h\"}"}]}"#,
        )
        .unwrap();
        assert_eq!(parsed.text, "done");
        assert_eq!(parsed.reasoning_content.as_deref(), Some("inspect"));
        assert_eq!(parsed.tool_calls[0].name, "execute_command");
        assert_eq!(parsed.tool_calls[0].arguments["command"], "df -h");
    }

    #[test]
    fn parses_sse_text_reasoning_tool_and_done_events() {
        let chunk = concat!(
            "event: response.output_text.delta\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\n\n",
            "data: {\"type\":\"response.reasoning_summary_text.delta\",\"delta\":\"think\"}\n\n",
            "data: {\"type\":\"response.function_call_arguments.delta\",\"output_index\":2,\"item_id\":\"call-1\",\"name\":\"execute_command\",\"delta\":\"{\\\"command\\\":\\\"df -h\\\"}\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"output\":[]}}\n\n"
        );
        let deltas = parse_openai_responses_stream_chunk(chunk).unwrap();
        assert_eq!(deltas[0].text_delta, "hello");
        assert_eq!(deltas[1].reasoning_delta.as_deref(), Some("think"));
        assert_eq!(deltas[2].tool_call_deltas[0].index, 2);
        assert!(deltas.last().is_some_and(|delta| delta.done));
    }

    #[test]
    fn rejects_error_events_without_leaking_request_credentials() {
        let error = parse_openai_responses_stream_chunk(
            "event: error\ndata: {\"type\":\"error\",\"error\":{\"message\":\"bad request\"}}\n\n",
        )
        .unwrap_err();
        assert_eq!(
            error,
            AiModelError::ResponsesError("bad request".to_string())
        );
    }
}
