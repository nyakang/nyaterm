//! Small, request-local supplements to SDK normalization. No SSE decoding here.
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use genai::chat::{ChatFrameSink, FrameCtx, RawFrameRef};
use nyaterm_core::ai::AiProviderApiProtocol;
use nyaterm_core::ai::completion::{
    AiStreamEventKind, AiStreamFrameFailure, AiStreamFrameFailureReason as FailureReason,
};
use serde_json::{Value, json};

const MAX_CONTINUATION_BYTES: usize = 32 * 1024 * 1024;
const MAX_TOOL_CALLS: usize = 128;

pub(super) struct FrameState {
    pub frame_count: usize,
    pub terminal: bool,
    pub failed: bool,
    pub failure: Option<AiStreamFrameFailure>,
    pub generated: bool,
    pub indexes: BTreeMap<String, usize>,
    pub items: BTreeMap<usize, Value>,
    pub data: Value,
    bytes: usize,
    arguments: BTreeMap<usize, String>,
    current_event: AiStreamEventKind,
    current_frame_bytes: usize,
}

impl Default for FrameState {
    fn default() -> Self {
        Self {
            frame_count: 0,
            terminal: false,
            failed: false,
            failure: None,
            generated: false,
            indexes: BTreeMap::new(),
            items: BTreeMap::new(),
            data: json!({}),
            bytes: 0,
            arguments: BTreeMap::new(),
            current_event: AiStreamEventKind::Other,
            current_frame_bytes: 0,
        }
    }
}

impl FrameState {
    fn fail(&mut self, reason: FailureReason) {
        self.failed = true;
        self.failure.get_or_insert(AiStreamFrameFailure {
            reason,
            frame: self.frame_count,
            event: self.current_event,
            data_bytes: self.current_frame_bytes,
        });
    }
}

fn event_kind(name: Option<&str>) -> AiStreamEventKind {
    match name {
        Some("response.created" | "response.in_progress" | "message_start" | "message_delta") => {
            AiStreamEventKind::Lifecycle
        }
        Some("response.output_item.added" | "content_block_start") => {
            AiStreamEventKind::OutputItemAdded
        }
        Some("response.output_item.done" | "content_block_stop") => {
            AiStreamEventKind::OutputItemDone
        }
        Some(
            "response.function_call_arguments.delta" | "response.function_call_arguments.done",
        ) => AiStreamEventKind::FunctionArguments,
        Some("response.output_text.delta" | "response.output_text.done") => AiStreamEventKind::Text,
        Some(
            "response.reasoning_text.delta"
            | "response.reasoning_summary_text.delta"
            | "response.reasoning_summary_text.done"
            | "response.reasoning_summary_part.added"
            | "response.reasoning_summary_part.done",
        ) => AiStreamEventKind::Reasoning,
        Some("response.completed" | "message_stop") => AiStreamEventKind::Completed,
        Some("response.incomplete") => AiStreamEventKind::Incomplete,
        Some("response.failed") => AiStreamEventKind::Failed,
        Some("error") => AiStreamEventKind::Error,
        _ => AiStreamEventKind::Other,
    }
}

pub(super) struct FrameObserver {
    pub state: Arc<Mutex<FrameState>>,
    protocol: AiProviderApiProtocol,
    responses: bool,
}

impl FrameObserver {
    pub fn new(protocol: AiProviderApiProtocol, responses: bool) -> Self {
        Self {
            state: Arc::new(Mutex::new(FrameState::default())),
            protocol,
            responses,
        }
    }
}

impl ChatFrameSink for FrameObserver {
    fn on_frame(&self, _context: &FrameCtx, frame: RawFrameRef<'_>) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        state.frame_count = state.frame_count.saturating_add(1);
        if state.terminal || state.failed {
            return;
        }
        state.current_event = event_kind(frame.event);
        state.current_frame_bytes = frame.data.len();
        // genai taps SSE blocks before discarding empty data (including comments
        // and keep-alives). Mirror that transport rule without accepting an end.
        // Ollama uses JSON lines, so its payloads still require valid JSON.
        if frame.data.is_empty() && self.protocol != AiProviderApiProtocol::Ollama {
            return;
        }
        if frame.data.trim() == "[DONE]" {
            state.terminal |=
                !self.responses && self.protocol == AiProviderApiProtocol::OpenaiCompatible;
            return;
        }
        let Some(mut value) = frame.json() else {
            state.fail(FailureReason::InvalidJson);
            return;
        };
        if !value.is_object() {
            state.fail(FailureReason::InvalidEnvelope);
            return;
        }
        if let Some(kind) = value.get("type").and_then(Value::as_str) {
            state.current_event = event_kind(Some(kind));
        }
        // Some compatible gateways include a nullable error field on success.
        // Explicit error events and non-null error payloads must still fail closed.
        if value.get("error").is_some_and(|error| !error.is_null())
            || matches!(
                value.get("type").and_then(Value::as_str),
                Some("error" | "response.failed")
            )
        {
            state.fail(FailureReason::ProviderError);
            return;
        }
        state.bytes = state.bytes.saturating_add(frame.data.len());
        if state.bytes > MAX_CONTINUATION_BYTES {
            state.fail(FailureReason::ContinuationSizeLimit);
            return;
        }
        if self.responses {
            let kind = value["type"].as_str().unwrap_or_default().to_string();
            state.generated |= kind.ends_with(".delta") || kind == "response.output_item.added";
            match kind.as_str() {
                "response.completed" | "response.incomplete" => {
                    state.terminal = true;
                    if !value["response"].is_object() {
                        state.fail(FailureReason::MissingResponseObject);
                        return;
                    }
                    state.data = value["response"].take();
                    if state.data["output"].as_array().is_none_or(Vec::is_empty) {
                        state.data["output"] = json!(state.items.values().collect::<Vec<_>>());
                    }
                }
                "response.output_item.added" | "response.output_item.done" => {
                    if !value["item"].is_object() {
                        state.fail(FailureReason::MissingOutputItemObject);
                        return;
                    }
                    if let Some(index) = index(&value, "output_index") {
                        if let Some(id) = value["item"]["call_id"].as_str() {
                            state.indexes.insert(id.into(), index);
                        }
                        state.items.insert(index, value["item"].take());
                    }
                }
                "response.function_call_arguments.delta" => {
                    if let Some(index) = index(&value, "output_index")
                        && let Some(item) = state.items.get_mut(&index)
                    {
                        let text = item["arguments"].as_str().unwrap_or_default().to_string()
                            + value["delta"].as_str().unwrap_or_default();
                        item["arguments"] = json!(text);
                    }
                }
                _ => {}
            }
        } else {
            match self.protocol {
                AiProviderApiProtocol::OpenaiCompatible => {
                    if let Some(calls) = value
                        .pointer("/choices/0/delta/tool_calls")
                        .and_then(Value::as_array)
                    {
                        for call in calls {
                            if let (Some(id), Some(index)) =
                                (call["id"].as_str(), index(call, "index"))
                            {
                                state.indexes.insert(id.into(), index);
                            }
                        }
                    }
                    state.generated |= value.pointer("/choices/0/delta").is_some_and(|delta| {
                        delta.as_object().is_some_and(|object| !object.is_empty())
                    });
                    if let Some(choice) = value.pointer("/choices/0").and_then(Value::as_object) {
                        if !state.data["choices"].is_array() {
                            state.data["choices"] = json!([{}]);
                        }
                        for (key, value) in choice {
                            if key != "delta" {
                                state.data["choices"][0][key] = value.clone();
                            }
                        }
                        if let Some(delta) = choice.get("delta").and_then(Value::as_object) {
                            for (key, value) in delta {
                                if !matches!(
                                    key.as_str(),
                                    "content" | "reasoning_content" | "tool_calls"
                                ) {
                                    state.data["choices"][0]["delta"][key] = value.clone();
                                }
                            }
                        }
                    }
                    if let Some(fields) = value.as_object() {
                        for (key, value) in fields {
                            if key != "choices" {
                                state.data[key] = value.clone();
                            }
                        }
                    }
                }
                AiProviderApiProtocol::Gemini => {
                    state.terminal = value
                        .pointer("/candidates/0/finishReason")
                        .is_some_and(|reason| reason.is_string());
                    if let Some(parts) = value
                        .pointer("/candidates/0/content/parts")
                        .and_then(Value::as_array)
                    {
                        state.generated |= !parts.is_empty();
                        for part in parts {
                            append_gemini_part(&mut state.items, part);
                        }
                    }
                    let mut candidate = value
                        .pointer("/candidates/0")
                        .cloned()
                        .or_else(|| state.data.pointer("/candidates/0").cloned())
                        .unwrap_or_else(|| json!({}));
                    if !candidate.is_object() {
                        state.fail(FailureReason::InvalidContentBlock);
                        return;
                    }
                    candidate["content"] =
                        json!({"role":"model", "parts":state.items.values().collect::<Vec<_>>()});
                    if let Some(fields) = value.as_object() {
                        for (key, value) in fields {
                            if key != "candidates" {
                                state.data[key] = value.clone();
                            }
                        }
                    }
                    state.data["candidates"] = json!([candidate]);
                }
                AiProviderApiProtocol::Anthropic => {
                    let kind = value["type"].as_str().unwrap_or_default().to_string();
                    state.terminal = kind == "message_stop";
                    state.generated |=
                        matches!(kind.as_str(), "content_block_start" | "content_block_delta");
                    if kind == "message_start" {
                        if !value["message"].is_object() {
                            state.fail(FailureReason::InvalidContentBlock);
                            return;
                        }
                        state.data = value["message"].take();
                    }
                    if kind == "message_delta" {
                        if let Some(fields) = value["delta"].as_object() {
                            for (key, value) in fields {
                                state.data[key] = value.clone();
                            }
                        }
                        if let Some(fields) = value["usage"].as_object() {
                            if !state.data["usage"].is_object() {
                                state.data["usage"] = json!({});
                            }
                            for (key, value) in fields {
                                state.data["usage"][key] = value.clone();
                            }
                        }
                    }
                    if let Some(index) = index(&value, "index") {
                        match kind.as_str() {
                            "content_block_start" => {
                                if state.items.contains_key(&index) {
                                    state.fail(FailureReason::DuplicateContentBlock);
                                    return;
                                }
                                let block = value["content_block"].clone();
                                if !block.is_object() {
                                    state.fail(FailureReason::InvalidContentBlock);
                                    return;
                                }
                                if let Some(id) = block["id"].as_str() {
                                    state.indexes.insert(id.to_string(), index);
                                }
                                state.items.insert(index, block);
                            }
                            "content_block_delta" => {
                                let delta = &value["delta"];
                                if delta["type"] == "input_json_delta" {
                                    state.arguments.entry(index).or_default().push_str(
                                        delta["partial_json"].as_str().unwrap_or_default(),
                                    );
                                } else if let Some(block) = state.items.get_mut(&index) {
                                    for field in ["text", "thinking", "signature"] {
                                        if let Some(fragment) = delta[field].as_str() {
                                            block[field] = json!(
                                                block[field]
                                                    .as_str()
                                                    .unwrap_or_default()
                                                    .to_string()
                                                    + fragment
                                            );
                                        }
                                    }
                                } else {
                                    state.fail(FailureReason::MissingContentBlock);
                                }
                            }
                            "content_block_stop" => {
                                if let Some(arguments) = state.arguments.remove(&index) {
                                    match serde_json::from_str::<Value>(&arguments) {
                                        Ok(input) if input.is_object() => {
                                            if let Some(block) = state.items.get_mut(&index) {
                                                block["input"] = input;
                                            }
                                        }
                                        _ => state.fail(FailureReason::InvalidToolArguments),
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    if state.terminal {
                        if !state.arguments.is_empty() {
                            state.fail(FailureReason::UnclosedToolArguments);
                        }
                        state.data["content"] = json!(state.items.values().collect::<Vec<_>>());
                    }
                }
                AiProviderApiProtocol::Ollama => {
                    state.generated |= value.get("message").is_some();
                    state.terminal = value["done"] == true;
                    if state.terminal {
                        state.data = value;
                    }
                }
            }
        }
        if state.indexes.len() > MAX_TOOL_CALLS || state.items.len() > 4096 {
            state.fail(FailureReason::StructureLimit);
        }
    }
}

fn index(value: &Value, key: &str) -> Option<usize> {
    value[key]
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
}

fn append_gemini_part(items: &mut BTreeMap<usize, Value>, part: &Value) {
    // A text part can span thousands of frames. Coalesce only unsigned adjacent
    // fragments with identical metadata, preserving signed/tool part boundaries.
    if let Some(fragment) = part.get("text").and_then(Value::as_str)
        && let Some((_, previous)) = items.last_key_value()
        && let (Some(previous), Some(current)) = (previous.as_object(), part.as_object())
        && !previous.contains_key("thoughtSignature")
        && !current.contains_key("thoughtSignature")
        && previous.len() == current.len()
        && current
            .iter()
            .all(|(key, value)| key == "text" || previous.get(key) == Some(value))
        && let Some(mut entry) = items.last_entry()
        && let Some(Value::String(text)) = entry.get_mut().get_mut("text")
    {
        text.push_str(fragment);
        return;
    }
    items.insert(items.len(), part.clone());
}

#[cfg(test)]
mod tests {
    use super::FrameObserver;
    use genai::chat::{ChatFrameSink, FrameCtx, RawFrameRef};
    use genai::{ModelIden, adapter::AdapterKind};
    use nyaterm_core::ai::AiProviderApiProtocol;
    use nyaterm_core::ai::completion::{AiStreamEventKind, AiStreamFrameFailureReason};
    use serde_json::json;

    fn observe(observer: &FrameObserver, data: &str) {
        observer.on_frame(
            &FrameCtx {
                stream_id: 1,
                model_iden: ModelIden::new(AdapterKind::OpenAI, "fixture"),
            },
            RawFrameRef {
                index: 0,
                event: None,
                data,
                elapsed_us: 0,
            },
        );
    }

    #[test]
    fn gemini_long_text_stream_coalesces_fragments_without_merging_signatures() {
        let observer = FrameObserver::new(AiProviderApiProtocol::Gemini, false);
        let frame = json!({"candidates":[{"content":{"parts":[{"text":"中"}]}}]}).to_string();
        for _ in 0..5000 {
            observe(&observer, &frame);
        }
        observe(&observer, &json!({"candidates":[{"content":{"parts":[{"text":"a","thoughtSignature":"s1"},{"text":"b","thoughtSignature":"s2"}]},"finishReason":"STOP"}]}).to_string());
        let state = observer.state.lock().unwrap();
        assert!(!state.failed);
        assert!(state.terminal);
        assert_eq!(state.items.len(), 3);
        assert_eq!(state.items[&0]["text"], "中".repeat(5000));
        assert_eq!(state.items[&1]["thoughtSignature"], "s1");
        assert_eq!(state.items[&2]["thoughtSignature"], "s2");
    }
    #[test]
    fn malformed_structured_frames_fail_without_panicking() {
        for data in [
            "null",
            "[]",
            "7",
            r#"{"type":"response.completed","response":7}"#,
            r#"{"type":"response.output_item.added","item":7}"#,
        ] {
            let observer = FrameObserver::new(AiProviderApiProtocol::OpenaiCompatible, true);
            observe(&observer, data);
            assert!(observer.state.lock().unwrap().failed);
        }
        let observer = FrameObserver::new(AiProviderApiProtocol::Anthropic, false);
        observe(&observer, r#"{"type":"message_start","message":7}"#);
        assert!(observer.state.lock().unwrap().failed);
    }

    #[test]
    fn responses_nullable_error_fields_do_not_reject_successful_events() {
        let observer = FrameObserver::new(AiProviderApiProtocol::OpenaiCompatible, true);
        for _ in 0..103 {
            observe(
                &observer,
                r#"{"type":"response.reasoning_summary_text.delta","delta":"private-reasoning","error":null}"#,
            );
        }
        observe(
            &observer,
            r#"{"type":"response.completed","response":{"output":[]},"error":null}"#,
        );
        let state = observer.state.lock().unwrap();
        assert_eq!(state.frame_count, 104);
        assert!(state.terminal && state.generated && !state.failed);
        assert!(state.failure.is_none());
    }

    #[test]
    fn first_frame_failure_retains_its_safe_cause_after_later_frames() {
        use AiStreamFrameFailureReason as Reason;
        for (data, reason, event) in [
            (
                "private-invalid-json",
                Reason::InvalidJson,
                AiStreamEventKind::Other,
            ),
            ("[]", Reason::InvalidEnvelope, AiStreamEventKind::Other),
            (
                r#"{"type":"response.output_item.done","item":null,"private-key":"private-content"}"#,
                Reason::MissingOutputItemObject,
                AiStreamEventKind::OutputItemDone,
            ),
            (
                r#"{"type":"response.completed","response":null}"#,
                Reason::MissingResponseObject,
                AiStreamEventKind::Completed,
            ),
            (
                r#"{"type":"response.failed","response":{"error":{"message":"private-content"}}}"#,
                Reason::ProviderError,
                AiStreamEventKind::Failed,
            ),
            (
                r#"{"type":"private-event-name","error":{"message":"private-content"}}"#,
                Reason::ProviderError,
                AiStreamEventKind::Other,
            ),
            (
                r#"{"type":"error","error":null}"#,
                Reason::ProviderError,
                AiStreamEventKind::Error,
            ),
        ] {
            let observer = FrameObserver::new(AiProviderApiProtocol::OpenaiCompatible, true);
            observe(&observer, r#"{"type":"response.created"}"#);
            observe(&observer, data);
            observe(
                &observer,
                r#"{"type":"response.completed","response":{"output":[]}}"#,
            );
            let state = observer.state.lock().unwrap();
            let failure = state.failure.unwrap();
            assert!(state.failed);
            assert_eq!(failure.reason, reason);
            assert_eq!(failure.event, event);
            assert_eq!(failure.frame, 2);
            assert_eq!(failure.data_bytes, data.len());
            assert!(!format!("{failure:?}").contains("private"));
        }
    }
    #[test]
    fn gemini_metadata_only_frames_preserve_raw_usage_without_synthetic_completion() {
        let observer = FrameObserver::new(AiProviderApiProtocol::Gemini, false);
        observe(
            &observer,
            r#"{"usageMetadata":{"promptTokenCount":2,"unknown":3}}"#,
        );
        let state = observer.state.lock().unwrap();
        assert!(!state.failed && !state.terminal);
        assert_eq!(
            state.data["usageMetadata"],
            json!({"promptTokenCount":2,"unknown":3})
        );
    }
    #[test]
    fn openai_unknown_choice_and_delta_fields_remain_in_metadata() {
        let observer = FrameObserver::new(AiProviderApiProtocol::OpenaiCompatible, false);
        observe(
            &observer,
            r#"{"choices":[{"index":0,"opaque":{"a":1},"delta":{"extension":{"b":2}}}]}"#,
        );
        let state = observer.state.lock().unwrap();
        assert_eq!(state.data["choices"][0]["opaque"], json!({"a":1}));
        assert_eq!(
            state.data["choices"][0]["delta"]["extension"],
            json!({"b":2})
        );
    }
}
