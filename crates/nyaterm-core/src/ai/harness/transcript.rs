use serde_json::{Value, json};

use super::AgentRequestContext;
use crate::ai::completion::AiContentPart;

pub fn append_openai(
    messages: &mut Vec<Value>,
    context: &AgentRequestContext,
    json_protocol: bool,
) {
    append_openai_with_language(messages, context, json_protocol, "en");
}

pub fn append_openai_with_language(
    messages: &mut Vec<Value>,
    context: &AgentRequestContext,
    json_protocol: bool,
    language: &str,
) {
    for turn in &context.turns {
        if json_protocol {
            for (entry, result) in turn.batch.calls.iter().zip(&turn.results) {
                messages.push(json!({"role":"assistant","content":json!({"action":entry.source.name,"arguments":entry.source.arguments}).to_string()}));
                messages.push(json!({"role":"user","content":crate::ai::build_agent_tool_result_message(&result.call_id, &result.value, language)}));
            }
        } else {
            let mut text = String::new();
            let mut reasoning = String::new();
            for part in &turn.batch.assistant_turn.parts {
                match part {
                    AiContentPart::Text(value) => text.push_str(value),
                    AiContentPart::Reasoning(value) => reasoning.push_str(value),
                    _ => {}
                }
            }
            let calls: Vec<Value> = turn.batch.calls.iter().map(|entry| json!({
                "id":entry.source.id,"type":"function","function":{"name":entry.source.name,"arguments":entry.source.arguments.to_string()}
            })).collect();
            let mut message = json!({"role":"assistant","content":if text.is_empty() { Value::Null } else { json!(text) },"tool_calls":calls});
            if !reasoning.is_empty() {
                message["reasoning_content"] = json!(reasoning);
            }
            messages.push(message);
            for result in &turn.results {
                messages.push(json!({"role":"tool","tool_call_id":result.call_id,"content":result.value.to_string()}));
            }
        }
    }
}

pub fn append_anthropic(
    messages: &mut Vec<Value>,
    context: &AgentRequestContext,
    json_protocol: bool,
) {
    append_anthropic_with_language(messages, context, json_protocol, "en");
}

pub fn append_anthropic_with_language(
    messages: &mut Vec<Value>,
    context: &AgentRequestContext,
    json_protocol: bool,
    language: &str,
) {
    if json_protocol {
        append_openai_with_language(messages, context, true, language);
        return;
    }
    for turn in &context.turns {
        let content = if turn.batch.assistant_turn.provider_items.is_empty() {
            turn.batch.calls.iter().map(|entry| json!({"type":"tool_use","id":entry.source.id,"name":entry.source.name,"input":entry.source.arguments})).collect::<Vec<_>>()
        } else {
            turn.batch.assistant_turn.provider_items.clone()
        };
        messages.push(json!({"role":"assistant","content":content}));
        messages.push(json!({"role":"user","content":turn.results.iter().map(|result| json!({"type":"tool_result","tool_use_id":result.call_id,"content":result.value.to_string(),"is_error":result.is_error})).collect::<Vec<_>>()}));
    }
}

pub fn append_gemini(
    contents: &mut Vec<Value>,
    context: &AgentRequestContext,
    json_protocol: bool,
) {
    append_gemini_with_language(contents, context, json_protocol, "en");
}

pub fn append_gemini_with_language(
    contents: &mut Vec<Value>,
    context: &AgentRequestContext,
    json_protocol: bool,
    language: &str,
) {
    for turn in &context.turns {
        if json_protocol {
            for (entry, result) in turn.batch.calls.iter().zip(&turn.results) {
                contents.push(json!({"role":"model","parts":[{"text":json!({"action":entry.source.name,"arguments":entry.source.arguments}).to_string()}]}));
                contents.push(json!({"role":"user","parts":[{"text":crate::ai::build_agent_tool_result_message(&result.call_id, &result.value, language)}]}));
            }
        } else {
            let parts = if turn.batch.assistant_turn.provider_items.is_empty() {
                turn.batch.calls.iter().map(|entry| {
                    let mut part = json!({"functionCall":{"name":entry.source.name,"args":entry.source.arguments}});
                    if let Some(signature) = &entry.source.thought_signature { part["thoughtSignature"] = json!(signature); }
                    part
                }).collect::<Vec<_>>()
            } else {
                turn.batch.assistant_turn.provider_items.clone()
            };
            let results: Vec<Value> = turn.batch.calls.iter().zip(&turn.results).enumerate().map(|(index, (entry, result))| {
                let mut response = json!({"name":entry.source.name,"response":{"result":result.value,"isError":result.is_error}});
                if let Some(id) = parts.iter().filter_map(|part| part.get("functionCall")).nth(index).and_then(|call| call.get("id")) { response["id"] = id.clone(); }
                json!({"functionResponse":response})
            }).collect();
            contents.push(json!({"role":"model","parts":parts}));
            contents.push(json!({"role":"user","parts":results}));
        }
    }
}

pub fn responses_input(messages: &[Value]) -> Vec<Value> {
    let mut input = Vec::new();
    for message in messages {
        if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
            if message["content"]
                .as_str()
                .is_some_and(|text| !text.is_empty())
            {
                input.push(json!({"role":"assistant","content":message["content"]}));
            }
            for call in calls {
                input.push(json!({"type":"function_call","call_id":call["id"],"name":call["function"]["name"],"arguments":call["function"]["arguments"]}));
            }
        } else if message["role"] == "tool" {
            input.push(json!({"type":"function_call_output","call_id":message["tool_call_id"],"output":message["content"]}));
        } else {
            input.push(message.clone());
        }
    }
    input
}

pub fn append_responses(input: &mut Vec<Value>, context: &AgentRequestContext) {
    for turn in &context.turns {
        if !turn.batch.assistant_turn.provider_items.is_empty() {
            input.extend(turn.batch.assistant_turn.provider_items.iter().cloned());
        } else {
            let mut messages = Vec::new();
            append_openai(
                &mut messages,
                &AgentRequestContext {
                    run_id: context.run_id.clone(),
                    turns: vec![turn.clone()],
                    remaining_steps: context.remaining_steps,
                },
                false,
            );
            input.extend(responses_input(&messages));
            continue;
        }
        input.extend(turn.results.iter().map(|result| json!({"type":"function_call_output","call_id":result.call_id,"output":result.value.to_string()})));
    }
}

/// OpenAI strict schemas require every property to be present; optional values
/// become nullable. The authoritative capability schema remains unchanged.
pub fn strict_schema(schema: &mut Value) {
    if let Some(object) = schema.as_object_mut() {
        object.remove("$schema");
    }
    let original_required = schema
        .get("required")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
        let names: Vec<String> = properties.keys().cloned().collect();
        for (name, property) in properties.iter_mut() {
            strict_schema(property);
            if !original_required
                .iter()
                .any(|required| required.as_str() == Some(name))
                && !allows_null(property)
            {
                *property = json!({"anyOf":[property.clone(),{"type":"null"}]});
            }
        }
        schema["required"] = schema_required(names);
        schema["additionalProperties"] = json!(false);
    }
    if let Some(items) = schema.get_mut("items") {
        strict_schema(items);
    }
    for key in ["anyOf", "oneOf", "allOf"] {
        if let Some(items) = schema.get_mut(key).and_then(Value::as_array_mut) {
            for item in items {
                strict_schema(item);
            }
        }
    }
    if let Some(defs) = schema.get_mut("$defs").and_then(Value::as_object_mut) {
        for value in defs.values_mut() {
            strict_schema(value);
        }
    }
}

fn schema_required(names: Vec<String>) -> Value {
    json!(names)
}
fn allows_null(schema: &Value) -> bool {
    schema["type"] == "null"
        || schema
            .get("type")
            .and_then(Value::as_array)
            .is_some_and(|types| types.iter().any(|ty| ty == "null"))
        || schema
            .get("anyOf")
            .and_then(Value::as_array)
            .is_some_and(|items| items.iter().any(allows_null))
}

#[cfg(test)]
mod tests {
    use super::strict_schema;
    use crate::ai::harness::{AgentRequestContext, AgentToolRegistry, AgentToolResult};
    use crate::ai::{
        AiApiFormat, AiBackendKind, AiChatRequest, AiProviderKind, AiSettings, ResolvedAiModel,
    };
    use serde_json::{Value, json};

    #[test]
    fn provider_requests_preserve_tool_results_and_ephemeral_gemini_signatures() {
        let mut call = AgentToolRegistry::parse(
            &[],
            r#"{"action":"session_get","arguments":{"sessionId":"target"}}"#,
        )
        .unwrap();
        call.id = "fixture-call".into();
        call.thought_signature = Some("fixture-signature".into());
        let mut request: AiChatRequest = serde_json::from_value(
            json!({"mode":"agent","action":"generate_command","userInput":"inspect"}),
        )
        .unwrap();
        request.options.agent_context = Some(AgentRequestContext {
            run_id: "fixture-run".into(),
            remaining_steps: 2,
            turns: vec![
                (
                    call,
                    AgentToolResult {
                        call_id: "fixture-call".into(),
                        value: json!({"code":"scope_denied"}),
                        is_error: true,
                    },
                )
                    .into(),
            ],
        });
        let settings = AiSettings::default();
        let model = ResolvedAiModel {
            model_name: "fixture-model".into(),
            backend: AiBackendKind::Genai,
            provider_kind: AiProviderKind::Openai,
            api_format: AiApiFormat::ChatCompletions,
            credential: None,
        };
        let chat =
            crate::ai::build_openai_compatible_chat_request_body(&model, &request, &settings, &[]);
        let messages = chat["messages"].as_array().unwrap();
        let result = messages
            .iter()
            .find(|message| message["role"] == "tool")
            .unwrap();
        assert_eq!(result["tool_call_id"], "fixture-call");
        assert_eq!(
            serde_json::from_str::<Value>(result["content"].as_str().unwrap()).unwrap()["code"],
            "scope_denied"
        );
        let responses = crate::ai::responses::build_openai_responses_request_body(
            &model,
            &request,
            &settings,
            &[],
            false,
        );
        let input = responses["input"].as_array().unwrap();
        assert_eq!(input[0], messages[0]);
        assert!(
            input[0]["content"]
                .as_str()
                .unwrap()
                .contains("Remaining tool decisions: 2.")
        );
        assert!(input[1..].iter().all(|item| item["role"] != "system"));
        assert!(
            input
                .iter()
                .any(|item| item["type"] == "function_call" && item["call_id"] == "fixture-call")
        );
        assert!(input.iter().any(
            |item| item["type"] == "function_call_output" && item["call_id"] == "fixture-call"
        ));
        let anthropic =
            crate::ai::build_anthropic_chat_request_body(&model, &request, &settings, &[]);
        let result = anthropic["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| message["content"][0]["type"] == "tool_result")
            .unwrap();
        assert_eq!(result["content"][0]["tool_use_id"], "fixture-call");
        assert_eq!(result["content"][0]["is_error"], true);
        let gemini = crate::ai::build_gemini_chat_request_body(&request, &settings, &[]);
        let contents = gemini["contents"].as_array().unwrap();
        assert!(
            contents
                .iter()
                .any(|content| content["parts"][0]["thoughtSignature"] == "fixture-signature")
        );
        assert!(
            contents.iter().any(
                |content| content["parts"][0]["functionResponse"]["response"]["isError"] == true
            )
        );
        let ollama = crate::ai::ollama::request_body(&model, &request, &settings, &[], false);
        let messages = ollama["messages"].as_array().unwrap();
        let assistant = messages
            .iter()
            .find(|message| message.get("tool_calls").is_some())
            .unwrap();
        assert!(assistant["tool_calls"][0]["function"]["arguments"].is_object());
        assert_eq!(
            messages
                .iter()
                .find(|message| message["role"] == "tool")
                .unwrap()["tool_name"],
            "session_get"
        );
        assert!(ollama["tools"][0]["function"].get("strict").is_none());
        let persisted = serde_json::to_value(&request).unwrap();
        assert!(!persisted.to_string().contains("fixture-signature"));
        assert!(
            !persisted["options"]
                .as_object()
                .unwrap()
                .contains_key("agentContext")
        );
        request
            .options
            .agent_context
            .as_mut()
            .unwrap()
            .remaining_steps = 0;
        let at_limit =
            crate::ai::build_openai_compatible_chat_request_body(&model, &request, &settings, &[]);
        assert_eq!(at_limit["tools"][0]["function"]["name"], "final_answer");
        assert_eq!(at_limit["tools"].as_array().unwrap().len(), 1);
        assert_eq!(at_limit["tool_choice"], "auto");
        assert!(at_limit.get("parallel_tool_calls").is_none());
        let at_limit = crate::ai::responses::build_openai_responses_request_body(
            &model,
            &request,
            &settings,
            &[],
            false,
        );
        assert_eq!(at_limit["tools"][0]["name"], "final_answer");
        assert_eq!(at_limit["tools"].as_array().unwrap().len(), 1);
        let at_limit =
            crate::ai::build_anthropic_chat_request_body(&model, &request, &settings, &[]);
        assert_eq!(at_limit["tools"][0]["name"], "final_answer");
        assert_eq!(at_limit["tools"].as_array().unwrap().len(), 1);
        assert_eq!(at_limit["tool_choice"]["type"], "auto");
        let at_limit = crate::ai::build_gemini_chat_request_body(&request, &settings, &[]);
        assert_eq!(
            at_limit["tools"][0]["functionDeclarations"][0]["name"],
            json!("final_answer")
        );
        request.options.agent_json_protocol = true;
        let fallback =
            crate::ai::build_openai_compatible_chat_request_body(&model, &request, &settings, &[]);
        assert!(fallback.get("tools").is_none());
        assert!(
            fallback["messages"]
                .as_array()
                .unwrap()
                .iter()
                .all(|message| message.get("tool_calls").is_none())
        );
        assert!(fallback.to_string().contains("scope_denied"));
        let ollama = crate::ai::ollama::request_body(&model, &request, &settings, &[], false);
        assert_eq!(ollama["messages"], fallback["messages"]);
        let responses = crate::ai::responses::build_openai_responses_request_body(
            &model,
            &request,
            &settings,
            &[],
            false,
        );
        assert_eq!(responses["input"], fallback["messages"]);
        let anthropic =
            crate::ai::build_anthropic_chat_request_body(&model, &request, &settings, &[]);
        assert!(anthropic["system"].is_string());
        assert!(
            anthropic["messages"]
                .as_array()
                .unwrap()
                .iter()
                .all(|message| message["role"] != "system")
        );
        assert!(anthropic["messages"].to_string().contains("scope_denied"));
    }

    #[test]
    fn strict_provider_schema_requires_nullable_optional_fields_recursively() {
        let mut schema = json!({"type":"object","properties":{"required":{"type":"string"},"optional":{"type":"object","properties":{"nested":{"type":"string"}}}},"required":["required"]});
        strict_schema(&mut schema);
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["required"].as_array().unwrap().len(), 2);
        assert_eq!(schema["properties"]["optional"]["anyOf"][1]["type"], "null");
        assert_eq!(
            schema["properties"]["optional"]["anyOf"][0]["properties"]["nested"]["anyOf"][1]["type"],
            "null"
        );
    }
}
