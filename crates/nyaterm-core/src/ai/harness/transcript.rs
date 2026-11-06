use serde_json::{Value, json};

use super::AgentRequestContext;

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
    for (call, result) in &context.calls {
        if json_protocol {
            messages.push(json!({"role":"assistant","content":serde_json::to_string(&json!({"action":call.tool.name(),"arguments":call.arguments,"thought":call.thought})).expect("agent action")}));
            messages.push(json!({"role":"user","content":crate::ai::build_agent_tool_result_message(&call.id, &result.value, language)}));
        } else {
            messages.push(json!({"role":"assistant", "content":null, "tool_calls":[{
                "id":call.id,"type":"function","function":{"name":call.tool.name(),"arguments":call.provider_arguments.to_string()}
            }]}));
            messages.push(
                json!({"role":"tool","tool_call_id":call.id,"content":result.value.to_string()}),
            );
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
    for (call, result) in &context.calls {
        messages.push(json!({"role":"assistant","content":[{"type":"tool_use","id":call.id,"name":call.tool.name(),"input":call.provider_arguments}]}));
        messages.push(json!({"role":"user","content":[{"type":"tool_result","tool_use_id":call.id,"content":result.value.to_string(),"is_error":result.is_error}]}));
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
    for (call, result) in &context.calls {
        if json_protocol {
            contents.push(json!({"role":"model","parts":[{"text":json!({"action":call.tool.name(),"arguments":call.arguments}).to_string()}]}));
            contents.push(json!({"role":"user","parts":[{"text":crate::ai::build_agent_tool_result_message(&call.id, &result.value, language)}]}));
        } else {
            let mut part =
                json!({"functionCall":{"name":call.tool.name(),"args":call.provider_arguments}});
            if let Some(signature) = &call.thought_signature {
                part["thoughtSignature"] = json!(signature);
            }
            contents.push(json!({"role":"model","parts":[part]}));
            contents.push(json!({"role":"user","parts":[{"functionResponse":{"name":call.tool.name(),"response":{"result":result.value,"isError":result.is_error}}}]}));
        }
    }
}

pub fn responses_input(messages: &[Value]) -> Vec<Value> {
    let mut input = Vec::new();
    for message in messages {
        if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
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
            calls: vec![(
                call,
                AgentToolResult {
                    call_id: "fixture-call".into(),
                    value: json!({"code":"scope_denied"}),
                    is_error: true,
                },
            )],
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
        assert_eq!(at_limit["tool_choice"]["function"]["name"], "final_answer");
        assert_eq!(at_limit["parallel_tool_calls"], false);
        let at_limit = crate::ai::responses::build_openai_responses_request_body(
            &model,
            &request,
            &settings,
            &[],
            false,
        );
        assert_eq!(at_limit["tool_choice"]["name"], "final_answer");
        let at_limit =
            crate::ai::build_anthropic_chat_request_body(&model, &request, &settings, &[]);
        assert_eq!(at_limit["tool_choice"]["name"], "final_answer");
        assert_eq!(at_limit["tool_choice"]["disable_parallel_tool_use"], true);
        let at_limit = crate::ai::build_gemini_chat_request_body(&request, &settings, &[]);
        assert_eq!(
            at_limit["toolConfig"]["functionCallingConfig"]["allowedFunctionNames"],
            json!(["final_answer"])
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
