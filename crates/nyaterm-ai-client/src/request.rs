use genai::chat::{
    ChatMessage, ChatOptions, ChatRequest, ContentPart, MessageContent, ReasoningEffort, Tool,
    ToolCall, ToolChoice, ToolResponse,
};
use nyaterm_core::ai::{
    AiChatRequest, AiMessage, AiMessageRole, AiProviderApiProtocol as Protocol, AiReasoningEffort,
    completion::{AiContentPart, AiProviderError, AiProviderErrorKind as Kind, request_messages},
    harness::AgentToolRegistry,
};
use serde_json::{Value, json};

use super::client::AiProviderSession;

pub(super) fn convert(
    session: &AiProviderSession,
    request: &AiChatRequest,
    history: &[AiMessage],
) -> Result<(ChatRequest, ChatOptions), AiProviderError> {
    let messages = request_messages(request, &session.settings, history);
    let mut chat = ChatRequest::new(
        messages
            .iter()
            .map(|(role, text)| match role {
                AiMessageRole::System => ChatMessage::system(text.clone()),
                AiMessageRole::User => ChatMessage::user(text.clone()),
                AiMessageRole::Assistant => ChatMessage::assistant(text.clone()),
            })
            .collect(),
    )
    .with_store(false);
    let generation = &request.options.generation;
    let mut extra = generation.extra_body.clone().unwrap_or_else(|| json!({}));
    if !extra.is_object()
        || [
            "model",
            "stream",
            "messages",
            "contents",
            "input",
            "tools",
            "tool_choice",
            "store",
            "previous_response_id",
        ]
        .iter()
        .any(|key| extra.get(key).is_some())
    {
        return Err(AiProviderError::new(Kind::InvalidRequest));
    }
    let mut options = ChatOptions::default()
        .with_capture_content(true)
        .with_capture_reasoning_content(true)
        .with_capture_usage(true)
        .with_capture_tool_calls(true)
        .with_capture_raw_body(true)
        .with_normalize_reasoning_content(false);
    if let Some(temperature) = &generation.temperature {
        let temperature = temperature
            .as_f64()
            .filter(|value| value.is_finite() && *value >= 0.)
            .ok_or_else(|| AiProviderError::new(Kind::InvalidRequest))?;
        options = options.with_temperature(temperature);
    } else if session.protocol == Protocol::Gemini {
        options = options.with_temperature(0.);
    }
    if let Some(max) = generation.max_tokens {
        if max == 0 {
            return Err(AiProviderError::new(Kind::InvalidRequest));
        }
        options = options.with_max_tokens(max);
    } else if session.protocol == Protocol::Anthropic {
        options = options.with_max_tokens(4096);
    }
    let effort = if request.options.connectivity_test {
        &AiReasoningEffort::Auto
    } else {
        &session.settings.default_reasoning_effort
    };
    let reasoning = match effort {
        AiReasoningEffort::Auto | AiReasoningEffort::Ultra => None,
        AiReasoningEffort::None => Some(ReasoningEffort::Zero),
        AiReasoningEffort::Minimal => Some(ReasoningEffort::Minimal),
        AiReasoningEffort::Low => Some(ReasoningEffort::Low),
        AiReasoningEffort::Medium => Some(ReasoningEffort::Medium),
        AiReasoningEffort::High => Some(ReasoningEffort::High),
        AiReasoningEffort::XHigh => Some(ReasoningEffort::XHigh),
        AiReasoningEffort::Max => Some(ReasoningEffort::Max),
    };
    let thinking = generation.thinking_budget.is_some()
        || !matches!(effort, AiReasoningEffort::Auto | AiReasoningEffort::None);
    if let Some(budget) = generation.thinking_budget {
        if budget == 0 || (session.protocol == Protocol::Anthropic && budget < 1024) {
            return Err(AiProviderError::new(Kind::InvalidRequest));
        }
        if generation
            .max_tokens
            .is_some_and(|max| session.protocol == Protocol::Anthropic && max <= budget)
        {
            return Err(AiProviderError::new(Kind::InvalidRequest));
        }
        options = options.with_reasoning_effort(ReasoningEffort::Budget(budget));
        if session.protocol == Protocol::Anthropic && generation.max_tokens.is_none() {
            options = options.with_max_tokens(
                budget
                    .checked_add(4096)
                    .ok_or_else(|| AiProviderError::new(Kind::InvalidRequest))?,
            );
        }
    } else if let Some(reasoning) = reasoning {
        if session.protocol == Protocol::Anthropic && thinking && generation.max_tokens.is_none() {
            let budget = match effort {
                AiReasoningEffort::Minimal | AiReasoningEffort::Low => 1024,
                AiReasoningEffort::Medium => 8000,
                _ => 24000,
            };
            options = options.with_max_tokens(budget + 4096);
        }
        options = options.with_reasoning_effort(reasoning);
    }
    if session.protocol == Protocol::Anthropic && thinking && generation.thinking_budget.is_none() {
        let budget = match effort {
            AiReasoningEffort::Minimal | AiReasoningEffort::Low => 1024,
            AiReasoningEffort::Medium => 8000,
            _ => 24000,
        };
        if generation.max_tokens.is_some_and(|max| max <= budget) {
            return Err(AiProviderError::new(Kind::InvalidRequest));
        }
    }
    if matches!(effort, AiReasoningEffort::Ultra) {
        if session.responses {
            extra["reasoning"] = json!({"effort":"ultra"});
        } else if session.protocol == Protocol::OpenaiCompatible {
            extra["reasoning_effort"] = json!("ultra");
        } else {
            return Err(AiProviderError::new(Kind::UnsupportedCapability));
        }
    }
    // Prevent SDK suffix/namespace parsing from renaming the selected model.
    if session.protocol != Protocol::Ollama && session.protocol != Protocol::Gemini {
        extra["model"] = json!(nyaterm_core::genai_model_name(
            &session.model.provider_kind,
            &session.model.model_name
        ));
    }
    let policy = AgentToolRegistry::request_policy(request, session.protocol);
    if let Some(tools) = policy.tools {
        chat.tools = Some(
            tools
                .iter()
                .map(|tool| {
                    let schema = AgentToolRegistry::schema(*tool);
                    let mut tool = Tool::new(tool.name())
                        .with_description(AgentToolRegistry::description(*tool))
                        .with_schema(schema);
                    tool.strict = policy.strict;
                    tool
                })
                .collect(),
        );
        options.tool_choice = Some(ToolChoice::Auto);
    }
    if let Some(context) = &request.options.agent_context {
        for turn in &context.turns {
            if turn.results.len() != turn.batch.calls.len()
                || turn
                    .batch
                    .calls
                    .iter()
                    .zip(&turn.results)
                    .any(|(entry, result)| {
                        entry.source.id.as_deref() != Some(result.call_id.as_str())
                    })
            {
                return Err(AiProviderError::new(Kind::InvalidRequest));
            }
            if request.options.agent_json_protocol {
                for (entry, result) in turn.batch.calls.iter().zip(&turn.results) {
                    chat.messages.push(ChatMessage::assistant(
                        json!({"action":entry.source.name,"arguments":entry.source.arguments})
                            .to_string(),
                    ));
                    chat.messages.push(ChatMessage::user(
                        nyaterm_core::ai::build_agent_tool_result_message(
                            &result.call_id,
                            &result.value,
                            &request.options.language,
                        ),
                    ));
                }
                continue;
            }
            let original = &turn.batch.assistant_turn;
            if original.protocol.is_some()
                && (original.protocol != Some(session.protocol)
                    || original.responses != session.responses)
            {
                return Err(AiProviderError::new(Kind::InvalidRequest));
            }
            let content =
                if session.protocol == Protocol::Anthropic && !original.provider_items.is_empty() {
                    MessageContent::from_parts(
                        original
                            .provider_items
                            .iter()
                            .cloned()
                            .map(|part| ContentPart::from_custom(part, None))
                            .collect::<Vec<_>>(),
                    )
                } else if !original.parts.is_empty() {
                    MessageContent::from_parts(
                        original
                            .parts
                            .iter()
                            .map(to_sdk_part)
                            .collect::<Result<Vec<_>, _>>()?,
                    )
                } else {
                    MessageContent::from_tool_calls(
                        turn.batch
                            .calls
                            .iter()
                            .map(|entry| ToolCall {
                                call_id: entry.source.id.clone().expect("validated id"),
                                fn_name: entry.source.name.clone(),
                                fn_arguments: entry.source.arguments.clone(),
                                thought_signatures: entry
                                    .source
                                    .thought_signature
                                    .clone()
                                    .map(|signature| vec![signature]),
                            })
                            .collect(),
                    )
                };
            chat.messages.push(ChatMessage::assistant(content));
            if session.protocol == Protocol::Anthropic {
                chat.messages.push(ChatMessage::user(MessageContent::from_parts(turn.results.iter().map(|result|
                    ContentPart::from_custom(json!({"type":"tool_result","tool_use_id":result.call_id,"content":result.value.to_string(),"is_error":result.is_error}), None)
                ).collect::<Vec<_>>())));
            } else {
                for (entry, result) in turn.batch.calls.iter().zip(&turn.results) {
                    let value = if session.protocol == Protocol::Gemini {
                        json!({"result":result.value,"isError":result.is_error})
                    } else {
                        result.value.clone()
                    };
                    chat.messages.push(ChatMessage::from(
                        ToolResponse::new(&result.call_id, value.to_string())
                            .with_fn_name(&entry.source.name),
                    ));
                }
            }
        }
        // Echo opaque provider continuation once per original assistant turn.
        if !request.options.agent_json_protocol && !context.turns.is_empty() {
            if session.responses {
                let mut input: Vec<Value> = messages
                    .iter()
                    .map(|(role, text)| json!({"role":role_name(role),"content":text}))
                    .collect();
                nyaterm_core::ai::harness::transcript::append_responses(&mut input, context);
                extra["input"] = json!(input);
            } else if session.protocol == Protocol::Gemini {
                let mut contents: Vec<Value> = messages.iter().filter(|(role, _)| *role != AiMessageRole::System).map(|(role, text)| json!({"role":if *role == AiMessageRole::Assistant { "model" } else { "user" },"parts":[{"text":text}]})).collect();
                nyaterm_core::ai::harness::transcript::append_gemini(&mut contents, context, false);
                extra["contents"] = json!(contents);
            }
        }
    }
    if request.options.connectivity_test {
        options.max_tokens = Some(64);
        options.reasoning_effort = None;
        chat.tools = None;
        options.tool_choice = None;
    }
    if !extra.as_object().expect("object").is_empty() {
        options.extra_body = Some(extra);
    }
    Ok((chat, options))
}

fn role_name(role: &AiMessageRole) -> &'static str {
    match role {
        AiMessageRole::System => "system",
        AiMessageRole::User => "user",
        AiMessageRole::Assistant => "assistant",
    }
}

pub(super) fn to_sdk_part(part: &AiContentPart) -> Result<ContentPart, AiProviderError> {
    Ok(match part {
        AiContentPart::Text(text) => ContentPart::Text(text.clone()),
        AiContentPart::Reasoning(text) => ContentPart::ReasoningContent(text.clone()),
        AiContentPart::Signature(signature) => ContentPart::ThoughtSignature(signature.clone()),
        AiContentPart::ToolCall {
            id,
            name,
            arguments,
            signatures,
        } => ContentPart::ToolCall(ToolCall {
            call_id: id.clone(),
            fn_name: name.clone(),
            fn_arguments: arguments.clone(),
            thought_signatures: (!signatures.is_empty()).then(|| signatures.clone()),
        }),
        AiContentPart::Custom(data) => ContentPart::from_custom(data.clone(), None),
        AiContentPart::Binary(data) => ContentPart::Binary(
            serde_json::from_value(data.clone())
                .map_err(|_| AiProviderError::new(Kind::Protocol))?,
        ),
    })
}
