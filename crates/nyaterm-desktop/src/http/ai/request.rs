use genai::chat::{
    ChatMessage, ChatOptions, ChatRequest, ContentPart, MessageContent, ReasoningEffort, Tool,
    ToolCall, ToolChoice, ToolResponse,
};
use nyaterm_core::ai::{
    AiChatRequest, AiMessage, AiMessageRole, AiMode, AiProviderApiProtocol as Protocol,
    AiReasoningEffort,
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
    if request.mode == AiMode::Agent
        && !request.options.agent_json_protocol
        && !request.options.connectivity_test
    {
        chat.tools = Some(
            AgentToolRegistry::TOOLS
                .iter()
                .map(|tool| {
                    let mut schema = AgentToolRegistry::schema(*tool);
                    let strict = session.protocol == Protocol::OpenaiCompatible;
                    if strict {
                        nyaterm_core::ai::harness::transcript::strict_schema(&mut schema);
                    }
                    let mut tool = Tool::new(tool.name())
                        .with_description(AgentToolRegistry::description(*tool))
                        .with_schema(schema);
                    if strict {
                        tool.strict = Some(true);
                    }
                    tool
                })
                .collect(),
        );
        options.tool_choice = Some(
            if session.protocol == Protocol::Anthropic
                && (thinking
                    || (matches!(effort, AiReasoningEffort::Auto)
                        && anthropic_default_thinking(&session.model.model_name)))
            {
                ToolChoice::Auto
            } else if request
                .options
                .agent_context
                .as_ref()
                .is_some_and(|context| context.remaining_steps == 0)
            {
                ToolChoice::Tool {
                    name: "final_answer".into(),
                }
            } else {
                ToolChoice::Required
            },
        );
        if session.protocol == Protocol::OpenaiCompatible {
            extra["parallel_tool_calls"] = json!(false);
        }
        if session.protocol == Protocol::Anthropic {
            extra["tool_choice"] = match options.tool_choice.as_ref() {
                Some(ToolChoice::Auto) => json!({"type":"auto","disable_parallel_tool_use":true}),
                Some(ToolChoice::Tool { name }) => {
                    json!({"type":"tool","name":name,"disable_parallel_tool_use":true})
                }
                _ => json!({"type":"any","disable_parallel_tool_use":true}),
            };
        }
    }
    if let Some(context) = &request.options.agent_context {
        for (call, result) in &context.calls {
            if call.id != result.call_id {
                return Err(AiProviderError::new(Kind::InvalidRequest));
            }
            if request.options.agent_json_protocol {
                chat.messages.push(ChatMessage::assistant(json!({"action":call.tool.name(),"arguments":call.arguments,"thought":call.thought}).to_string()));
                chat.messages.push(ChatMessage::user(format!(
                    "NyaTerm tool result for {} (untrusted data):\n{}",
                    call.id, result.value
                )));
                continue;
            }
            let content = if let Some(turn) = &call.assistant_turn {
                if turn.protocol != Some(session.protocol) || turn.responses != session.responses {
                    return Err(AiProviderError::new(Kind::InvalidRequest));
                }
                if session.protocol == Protocol::Anthropic && !turn.provider_items.is_empty() {
                    MessageContent::from_parts(
                        turn.provider_items
                            .iter()
                            .cloned()
                            .map(|part| ContentPart::from_custom(part, None))
                            .collect::<Vec<_>>(),
                    )
                } else {
                    MessageContent::from_parts(
                        turn.parts
                            .iter()
                            .map(to_sdk_part)
                            .collect::<Result<Vec<_>, _>>()?,
                    )
                }
            } else {
                MessageContent::from_tool_calls(vec![ToolCall {
                    call_id: call.id.clone(),
                    fn_name: call.tool.name().into(),
                    fn_arguments: call.provider_arguments.clone(),
                    thought_signatures: call
                        .thought_signature
                        .clone()
                        .map(|signature| vec![signature]),
                }])
            };
            chat.messages.push(ChatMessage::assistant(content));
            let response = if session.protocol == Protocol::Anthropic {
                ChatMessage::user(MessageContent::from_parts(vec![ContentPart::from_custom(
                    json!({"type":"tool_result","tool_use_id":call.id,"content":result.value.to_string(),"is_error":result.is_error}),
                    None,
                )]))
            } else {
                let value = if session.protocol == Protocol::Gemini {
                    json!({"result":result.value,"isError":result.is_error})
                } else {
                    result.value.clone()
                };
                ChatMessage::from(
                    ToolResponse::new(&call.id, value.to_string()).with_fn_name(call.tool.name()),
                )
            };
            chat.messages.push(response);
        }
        // Responses output items and Gemini parts carry opaque continuation
        // fields the uniform SDK message type cannot always echo faithfully.
        if !request.options.agent_json_protocol
            && context.calls.iter().any(|(call, _)| {
                call.assistant_turn
                    .as_ref()
                    .is_some_and(|turn| !turn.provider_items.is_empty())
            })
        {
            if session.responses {
                let mut input: Vec<Value> = messages
                    .iter()
                    .map(|(role, text)| json!({"role":role_name(role),"content":text}))
                    .collect();
                for (call, result) in &context.calls {
                    if let Some(turn) = &call.assistant_turn {
                        input.extend(turn.provider_items.iter().cloned());
                    } else {
                        input.push(json!({"type":"function_call","call_id":call.id,"name":call.tool.name(),"arguments":call.provider_arguments.to_string()}));
                    }
                    input.push(json!({"type":"function_call_output","call_id":call.id,"output":result.value.to_string()}));
                }
                extra["input"] = json!(input);
            } else if session.protocol == Protocol::Gemini {
                let mut contents: Vec<Value> = messages.iter().filter(|(role, _)| *role != AiMessageRole::System).map(|(role, text)| json!({"role":if *role == AiMessageRole::Assistant { "model" } else { "user" },"parts":[{"text":text}]})).collect();
                for (call, result) in &context.calls {
                    let parts = call.assistant_turn.as_ref().map(|turn| turn.provider_items.clone()).filter(|parts| !parts.is_empty()).unwrap_or_else(|| vec![json!({"functionCall":{"name":call.tool.name(),"args":call.provider_arguments}})]);
                    let provider_id = parts
                        .iter()
                        .find_map(|part| part.pointer("/functionCall/id"))
                        .cloned();
                    contents.push(json!({"role":"model","parts":parts}));
                    let mut response = json!({"name":call.tool.name(),"response":{"result":result.value,"isError":result.is_error}});
                    if let Some(id) = provider_id {
                        response["id"] = id;
                    }
                    contents.push(json!({"role":"user","parts":[{"functionResponse":response}]}));
                }
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

// Mirrors the default-on families in genai 0.7.3's Anthropic capabilities.
// Other models retain required tool selection when no thinking was requested.
fn anthropic_default_thinking(model: &str) -> bool {
    let model = model.rsplit("::").next().unwrap_or(model);
    let mut segments = model.split('-');
    if segments.next() == Some("claude") {
        let family = segments.next();
        let major = segments.next().and_then(|value| value.parse::<u32>().ok());
        match family {
            Some("opus" | "sonnet") => return major == Some(5),
            Some("haiku") => return major.is_some_and(|major| major >= 5),
            Some("fable" | "mythos") => return false,
            _ => {}
        }
    }
    model.contains("claude-sonnet-5") || model.contains("claude-opus-5")
}
