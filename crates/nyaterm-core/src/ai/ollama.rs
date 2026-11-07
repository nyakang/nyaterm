//! Native Ollama JSON and NDJSON contracts.

use super::{
    AiChatCompletion, AiChatRequest, AiChatStreamDelta, AiMessage, AiModelError, AiSettings,
    ResolvedAiModel,
};
use serde_json::{Value, json};

pub fn ollama_url(base: &str, path: &str) -> Result<String, AiModelError> {
    super::providers::join_api_base_url(base, path)
}

pub fn request_body(
    model: &ResolvedAiModel,
    request: &AiChatRequest,
    settings: &AiSettings,
    history: &[AiMessage],
    stream: bool,
) -> Value {
    let openai = super::build_openai_compatible_chat_request_body_with_stream(
        model, request, settings, history, stream,
    );
    let mut body =
        json!({"model": model.model_name, "messages": openai["messages"], "stream": stream});
    let mut names = std::collections::HashMap::new();
    for message in body["messages"].as_array_mut().expect("messages") {
        if let Some(calls) = message.get_mut("tool_calls").and_then(Value::as_array_mut) {
            for call in calls {
                if let (Some(id), Some(name)) =
                    (call["id"].as_str(), call["function"]["name"].as_str())
                {
                    names.insert(id.to_string(), name.to_string());
                }
                if let Some(arguments) = call["function"]["arguments"].as_str() {
                    // The shared transcript encoded a Value as JSON text.
                    call["function"]["arguments"] =
                        serde_json::from_str(arguments).expect("encoded arguments");
                }
            }
        }
        if message["role"] == "tool"
            && let Some(name) = message["tool_call_id"]
                .as_str()
                .and_then(|id| names.get(id))
        {
            message["tool_name"] = json!(name);
        }
        if let Some(reasoning) = message
            .as_object_mut()
            .expect("message")
            .remove("reasoning_content")
        {
            message["thinking"] = reasoning;
        }
    }
    if let Some(tools) = openai.get("tools") {
        body["tools"] = tools.clone();
        for tool in body["tools"].as_array_mut().expect("tools") {
            tool["function"]
                .as_object_mut()
                .expect("function")
                .remove("strict");
        }
    }
    if request.options.connectivity_test {
        body["options"] = json!({"num_predict": 64});
    }
    body
}

pub fn parse_response(raw: &str) -> Result<AiChatCompletion, AiModelError> {
    let mut value: Value = serde_json::from_str(raw)
        .map_err(|error| AiModelError::InvalidChatJson(error.to_string()))?;
    if value.get("error").is_some() {
        return Err(AiModelError::InvalidChatJson(
            "Ollama returned an error".into(),
        ));
    }
    if !value["message"].is_object() && value["done"] != true {
        return Err(AiModelError::MissingChatContent);
    }
    if let Some(thinking) = value["message"].get("thinking").cloned() {
        value["message"]["reasoning_content"] = thinking;
    }
    // Reuse the tool-call and reasoning parser after adapting Ollama's envelope.
    super::parse_openai_compatible_chat_response(
        &json!({"choices": [{"message": value["message"]}]}).to_string(),
    )
}

pub fn parse_stream(raw: &str) -> Result<Vec<AiChatStreamDelta>, AiModelError> {
    let mut value: Value = serde_json::from_str(raw)
        .map_err(|error| AiModelError::InvalidChatJson(error.to_string()))?;
    if value.get("error").is_some() {
        return Err(AiModelError::InvalidChatJson(
            "Ollama returned an error".into(),
        ));
    }
    if !value["message"].is_object() && value["done"] != true {
        return Err(AiModelError::MissingChatContent);
    }
    if let Some(thinking) = value["message"].get("thinking").cloned() {
        value["message"]["reasoning_content"] = thinking;
    }
    if let Some(calls) = value["message"]["tool_calls"].as_array_mut() {
        for (index, call) in calls.iter_mut().enumerate() {
            call["index"] = json!(index);
            if call["function"]["arguments"].is_object() {
                call["function"]["arguments"] = json!(call["function"]["arguments"].to_string());
            }
        }
    }
    let mut deltas = super::parse_openai_compatible_stream_chunk(&format!(
        "data: {}\n\n",
        json!({"choices": [{"delta": value["message"], "finish_reason": if value["done"] == true { json!("stop") } else { Value::Null }}]})
    ))?;
    if value["done"] == true && !deltas.iter().any(|delta| delta.done) {
        deltas.push(AiChatStreamDelta {
            done: true,
            ..Default::default()
        });
    }
    Ok(deltas)
}

#[cfg(test)]
mod tests {
    use super::{parse_response, parse_stream};
    #[test]
    fn native_ollama_parses_thinking_and_object_tool_arguments() {
        let completion = parse_response(r#"{"message":{"content":"OK","thinking":"check","tool_calls":[{"function":{"name":"execute_command","arguments":{"command":"pwd"}}}]},"done":true}"#).unwrap();
        assert_eq!(completion.text, "OK");
        assert_eq!(completion.reasoning_content.as_deref(), Some("check"));
        assert_eq!(completion.tool_calls[0].arguments["command"], "pwd");
        let delta =
            parse_stream(r#"{"message":{"content":"O","thinking":"check"},"done":false}"#).unwrap();
        assert_eq!(delta[0].text_delta, "O");
        assert_eq!(delta[0].reasoning_delta.as_deref(), Some("check"));
        let done = parse_stream(r#"{"done":true}"#).unwrap();
        assert_eq!(done.iter().filter(|delta| delta.done).count(), 1);
    }
    #[test]
    fn native_ollama_rejects_errors_and_invalid_envelopes() {
        assert!(parse_response(r#"{"error":"fixture failure"}"#).is_err());
        assert!(parse_stream(r#"{"error":"fixture failure"}"#).is_err());
        assert!(parse_stream("{}").is_err());
        assert!(parse_stream("not json").is_err());
    }
}
