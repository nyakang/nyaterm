use genai::chat::{ContentPart, MessageContent, Usage};
use nyaterm_core::ai::{
    AiChatCompletion, AiToolCall,
    completion::{
        AiAssistantTurn, AiContentPart, AiProviderDiagnosticCode, AiProviderError,
        AiResponseMetadata, AiUsage,
    },
};
use serde_json::Value;

use super::client::AiProviderSession;
use super::error::protocol_error;

pub(super) fn completion(
    session: &AiProviderSession,
    content: MessageContent,
    reasoning: Option<String>,
    usage: Option<Usage>,
    stop: Option<String>,
    response_id: Option<String>,
    provider_data: Value,
) -> Result<AiChatCompletion, AiProviderError> {
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    let mut parts = Vec::new();
    let mut ids = std::collections::HashSet::new();
    for part in content.into_parts() {
        parts.push(match part {
            ContentPart::Text(value) => {
                text.push_str(&value);
                AiContentPart::Text(value)
            }
            ContentPart::ReasoningContent(value) => AiContentPart::Reasoning(value),
            ContentPart::ThoughtSignature(value) => AiContentPart::Signature(value),
            ContentPart::Custom(value) => AiContentPart::Custom(value.data),
            ContentPart::Binary(value) => AiContentPart::Binary(
                serde_json::to_value(value)
                    .map_err(|_| protocol_error(AiProviderDiagnosticCode::InvalidCompletion))?,
            ),
            ContentPart::ToolCall(call) => {
                if call.call_id.is_empty()
                    || call.fn_name.is_empty()
                    || !ids.insert(call.call_id.clone())
                {
                    return Err(protocol_error(AiProviderDiagnosticCode::InvalidCompletion));
                }
                let signatures = call.thought_signatures.unwrap_or_default();
                tool_calls.push(AiToolCall {
                    id: Some(call.call_id.clone()),
                    name: call.fn_name.clone(),
                    arguments: call.fn_arguments.clone(),
                    thought_signature: signatures.first().cloned(),
                });
                AiContentPart::ToolCall {
                    id: call.call_id,
                    name: call.fn_name,
                    arguments: call.fn_arguments,
                    signatures,
                }
            }
            ContentPart::ToolResponse(_) => {
                return Err(protocol_error(AiProviderDiagnosticCode::InvalidCompletion));
            }
        });
    }
    let mut reasoning = reasoning;
    if reasoning.is_none() {
        let values: String = parts
            .iter()
            .filter_map(|part| match part {
                AiContentPart::Reasoning(value) => Some(value.as_str()),
                _ => None,
            })
            .collect();
        reasoning = (!values.is_empty()).then_some(values);
    } else if !parts
        .iter()
        .any(|part| matches!(part, AiContentPart::Reasoning(_)))
        && let Some(value) = &reasoning
    {
        parts.insert(0, AiContentPart::Reasoning(value.clone()));
    }
    if text.is_empty()
        && reasoning.is_none()
        && tool_calls.is_empty()
        && parts.is_empty()
        && stop.is_none()
        && provider_data.is_null()
    {
        return Err(protocol_error(AiProviderDiagnosticCode::InvalidCompletion));
    }
    let metadata = AiResponseMetadata {
        usage: usage.as_ref().map(convert_usage).unwrap_or_default(),
        finish_reason: stop,
        response_id: response_id.or_else(|| {
            provider_data
                .get("id")
                .or_else(|| provider_data.get("responseId"))
                .and_then(Value::as_str)
                .map(str::to_string)
        }),
        provider_model: provider_data
            .get("model")
            .or_else(|| provider_data.get("modelVersion"))
            .and_then(Value::as_str)
            .map(str::to_string),
        assistant_turn: AiAssistantTurn {
            protocol: Some(session.protocol),
            responses: session.responses,
            parts,
            provider_items: provider_items(session, &provider_data),
        },
        provider_data,
    };
    Ok(AiChatCompletion {
        text,
        reasoning_content: reasoning,
        tool_calls,
        metadata,
    })
}

pub(super) fn provider_items(session: &AiProviderSession, data: &Value) -> Vec<Value> {
    let items = if session.responses {
        data.get("output")
    } else {
        match session.protocol {
            nyaterm_core::ai::AiProviderApiProtocol::Gemini => {
                data.pointer("/candidates/0/content/parts")
            }
            nyaterm_core::ai::AiProviderApiProtocol::Anthropic => data.get("content"),
            _ => None,
        }
    };
    items.and_then(Value::as_array).cloned().unwrap_or_default()
}

fn count(value: Option<i32>) -> Option<u64> {
    value.and_then(|value| u64::try_from(value).ok())
}
fn convert_usage(usage: &Usage) -> AiUsage {
    AiUsage {
        input_tokens: count(usage.prompt_tokens),
        output_tokens: count(usage.completion_tokens),
        total_tokens: count(usage.total_tokens),
        reasoning_tokens: count(
            usage
                .completion_tokens_details
                .as_ref()
                .and_then(|details| details.reasoning_tokens),
        ),
        cached_tokens: count(
            usage
                .prompt_tokens_details
                .as_ref()
                .and_then(|details| details.cached_tokens),
        ),
        cache_creation_tokens: count(
            usage
                .prompt_tokens_details
                .as_ref()
                .and_then(|details| details.cache_creation_tokens),
        ),
    }
}
