//! Narrow native path: genai 0.7.3 strips `::` from Ollama model names and
//! ignores extra_body. Select this path before starting a run, never on error.
use std::sync::Arc;

use futures::StreamExt;
use nyaterm_core::ai::{
    AiChatCompletion, AiChatRequest, AiChatStreamDelta, AiMessage,
    completion::{AiContentPart, AiProviderError, AiProviderErrorKind as Kind},
};
use serde_json::{Value, json};
use tokio::sync::mpsc::Sender;

use super::{
    adapter::send_delta,
    client::AiProviderSession,
    error::{http_error, status_error},
};

pub(super) async fn complete(
    session: &Arc<AiProviderSession>,
    request: &AiChatRequest,
    history: &[AiMessage],
    tx: Option<&Sender<AiChatStreamDelta>>,
) -> Result<AiChatCompletion, AiProviderError> {
    // Run the same option validation as the SDK path.
    let (chat, options) = super::request::convert(session, request, history)?;
    let mut messages = Vec::new();
    for message in chat.messages {
        let role = match message.role {
            genai::chat::ChatRole::System => "system",
            genai::chat::ChatRole::User => "user",
            genai::chat::ChatRole::Assistant => "assistant",
            genai::chat::ChatRole::Tool => "tool",
        };
        let mut value = json!({"role":role,"content":""});
        let mut calls = Vec::new();
        let mut text = String::new();
        let mut reasoning = String::new();
        for part in message.content {
            match part {
                genai::chat::ContentPart::Text(content) => text.push_str(&content),
                genai::chat::ContentPart::ReasoningContent(content) => reasoning.push_str(&content),
                genai::chat::ContentPart::ToolCall(call) => calls.push(json!({"id":call.call_id,"type":"function","function":{"name":call.fn_name,"arguments":call.fn_arguments}})),
                genai::chat::ContentPart::ToolResponse(response) => { value["tool_name"] = json!(response.fn_name); text.push_str(&response.content); },
                _ => return Err(AiProviderError::new(Kind::UnsupportedCapability)),
            }
        }
        value["content"] = json!(text);
        if !reasoning.is_empty() {
            value["thinking"] = json!(reasoning);
        }
        if !calls.is_empty() {
            value["tool_calls"] = json!(calls);
        }
        messages.push(value);
    }
    let mut body =
        json!({"model":session.model.model_name,"messages":messages,"stream":tx.is_some()});
    if let Some(tools) = chat.tools {
        body["tools"] = json!(tools.into_iter().map(|tool| json!({"type":"function","function":{"name":tool.name,"description":tool.description,"parameters":tool.schema}})).collect::<Vec<_>>());
    }
    if let Some(extra) = &request.options.generation.extra_body {
        for (key, value) in extra
            .as_object()
            .ok_or_else(|| AiProviderError::new(Kind::InvalidRequest))?
        {
            body[key] = value.clone();
        }
    }
    if (options.temperature.is_some() || options.max_tokens.is_some())
        && !body["options"].is_object()
    {
        body["options"] = json!({});
    }
    if let Some(temperature) = options.temperature {
        body["options"]["temperature"] = json!(temperature);
    }
    if let Some(max) = options.max_tokens {
        body["options"]["num_predict"] = json!(max);
    }
    if let Some(effort) = options.reasoning_effort {
        body["think"] = match effort {
            genai::chat::ReasoningEffort::Zero => json!(false),
            genai::chat::ReasoningEffort::Low | genai::chat::ReasoningEffort::Minimal => {
                json!("low")
            }
            genai::chat::ReasoningEffort::Medium => json!("medium"),
            genai::chat::ReasoningEffort::High => json!("high"),
            genai::chat::ReasoningEffort::Max | genai::chat::ReasoningEffort::XHigh => json!("max"),
            genai::chat::ReasoningEffort::Budget(_) => {
                return Err(AiProviderError::new(Kind::UnsupportedCapability));
            }
        };
    }
    let genai::resolver::AuthData::RequestOverride { url, .. } = session.target(tx.is_some())?.auth
    else {
        unreachable!()
    };
    let credential = session
        .model
        .credential
        .as_ref()
        .expect("validated credential");
    let response = session
        .client()
        .await?
        .http
        .post(url)
        .headers(super::client::headers(credential, session.protocol)?)
        .json(&body)
        .send()
        .await
        .map_err(http_error)?;
    if !response.status().is_success() {
        return Err(status_error(
            response.status().as_u16(),
            response.headers(),
            None,
        ));
    }
    let mut completion = AiChatCompletion {
        text: String::new(),
        reasoning_content: None,
        tool_calls: Vec::new(),
        metadata: Default::default(),
    };
    if tx.is_none() {
        let raw: Value = response.json().await.map_err(http_error)?;
        if !process_line(&mut completion, raw, None).await? {
            return Err(AiProviderError::new(Kind::Protocol));
        }
        return Ok(completion);
    }
    let mut bytes = response.bytes_stream();
    let mut pending = Vec::new();
    while let Some(chunk) = bytes.next().await {
        pending.extend_from_slice(&chunk.map_err(http_error)?);
        if pending.len() > 4 * 1024 * 1024 {
            return Err(AiProviderError::new(Kind::Protocol));
        }
        while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
            let line: Vec<_> = pending.drain(..=end).collect();
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            let value: Value =
                serde_json::from_slice(&line).map_err(|_| AiProviderError::new(Kind::Protocol))?;
            if process_line(&mut completion, value, tx).await? {
                return Ok(completion);
            }
        }
    }
    if !pending.is_empty() {
        let value: Value =
            serde_json::from_slice(&pending).map_err(|_| AiProviderError::new(Kind::Protocol))?;
        if process_line(&mut completion, value, tx).await? {
            return Ok(completion);
        }
    }
    Err(AiProviderError::new(Kind::StreamInterrupted))
}

async fn process_line(
    result: &mut AiChatCompletion,
    value: Value,
    tx: Option<&Sender<AiChatStreamDelta>>,
) -> Result<bool, AiProviderError> {
    if value.get("error").is_some() || !value.is_object() {
        return Err(AiProviderError::new(Kind::Protocol));
    }
    let text = value
        .pointer("/message/content")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let reasoning = value
        .pointer("/message/thinking")
        .or_else(|| value.pointer("/message/reasoning_content"))
        .and_then(Value::as_str);
    result.text.push_str(text);
    if let Some(reasoning) = reasoning {
        result
            .reasoning_content
            .get_or_insert_default()
            .push_str(reasoning);
    }
    let mut delta = AiChatStreamDelta {
        text_delta: text.into(),
        reasoning_delta: reasoning.map(str::to_string),
        ..Default::default()
    };
    if let Some(calls) = value
        .pointer("/message/tool_calls")
        .and_then(Value::as_array)
    {
        for call in calls {
            if result.tool_calls.len() >= 128 {
                return Err(AiProviderError::new(Kind::Protocol));
            }
            let name = call
                .pointer("/function/name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .ok_or_else(|| AiProviderError::new(Kind::Protocol))?;
            let arguments = call
                .pointer("/function/arguments")
                .ok_or_else(|| AiProviderError::new(Kind::Protocol))?
                .clone();
            let id = call["id"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| format!("call-{}", uuid::Uuid::new_v4()));
            if result
                .tool_calls
                .iter()
                .any(|call| call.id.as_deref() == Some(&id))
            {
                return Err(AiProviderError::new(Kind::Protocol));
            }
            delta
                .tool_call_deltas
                .push(nyaterm_core::ai::AiToolCallDelta {
                    index: result.tool_calls.len(),
                    id_delta: Some(id.clone()),
                    name_delta: Some(name.into()),
                    arguments_delta: arguments.to_string(),
                    thought_signature: None,
                });
            result.tool_calls.push(nyaterm_core::ai::AiToolCall {
                id: Some(id),
                name: name.into(),
                arguments,
                thought_signature: None,
            });
        }
    }
    let done = value["done"] == true;
    if let Some(tx) = tx {
        send_delta(tx, delta).await?;
    }
    if done {
        fill_metadata(result, value);
        if let Some(tx) = tx {
            send_delta(
                tx,
                AiChatStreamDelta {
                    done: true,
                    metadata: Some(result.metadata.clone()),
                    ..Default::default()
                },
            )
            .await?;
        }
    }
    Ok(done)
}

fn fill_metadata(result: &mut AiChatCompletion, raw: Value) {
    result.metadata.usage.input_tokens = raw["prompt_eval_count"].as_u64();
    result.metadata.usage.output_tokens = raw["eval_count"].as_u64();
    result.metadata.usage.total_tokens = result
        .metadata
        .usage
        .input_tokens
        .zip(result.metadata.usage.output_tokens)
        .and_then(|(input, output)| input.checked_add(output));
    result.metadata.finish_reason = raw["done_reason"].as_str().map(str::to_string);
    result.metadata.provider_model = raw["model"].as_str().map(str::to_string);
    result.metadata.assistant_turn.protocol = Some(nyaterm_core::ai::AiProviderApiProtocol::Ollama);
    result.metadata.assistant_turn.parts = vec![AiContentPart::Text(result.text.clone())];
    if let Some(reasoning) = &result.reasoning_content {
        result
            .metadata
            .assistant_turn
            .parts
            .push(AiContentPart::Reasoning(reasoning.clone()));
    }
    for call in &result.tool_calls {
        result
            .metadata
            .assistant_turn
            .parts
            .push(AiContentPart::ToolCall {
                id: call.id.clone().unwrap_or_default(),
                name: call.name.clone(),
                arguments: call.arguments.clone(),
                signatures: Vec::new(),
            });
    }
    result.metadata.provider_data = raw;
}
