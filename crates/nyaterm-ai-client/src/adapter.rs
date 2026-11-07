use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use genai::chat::{ChatStreamEvent, ContentPart, MessageContent};
use nyaterm_core::ai::{
    AiChatCompletion, AiChatRequest, AiChatStreamDelta, AiMessage, AiToolCallDelta,
    completion::{
        AiProviderDiagnosticCode as Code, AiProviderError, AiProviderErrorKind as Kind,
        AiProviderFailureStage as Stage, AiStreamDiagnostic,
    },
};
use tokio::sync::mpsc::Sender;
use tokio_util::sync::CancellationToken;

use super::{
    client::AiProviderSession,
    error::{at_stage, protocol_error, sdk_error},
    frames::FrameObserver,
    request, response,
};

pub async fn complete_chat(
    session: &Arc<AiProviderSession>,
    request: &AiChatRequest,
    history: &[AiMessage],
    cancel: &CancellationToken,
) -> Result<AiChatCompletion, AiProviderError> {
    guarded(session, cancel, async {
        if session.ollama_compat {
            return super::ollama_compat::complete(session, request, history, None).await;
        }
        let (chat, options) = request::convert(session, request, history)
            .map_err(|error| at_stage(error, Stage::PrepareRequest))?;
        let response = session
            .client()
            .await?
            .sdk
            .exec_chat(session.target(false)?, chat, Some(&options))
            .await
            .map_err(|error| at_stage(sdk_error(error), Stage::ReadResponse))?;
        let raw = response.captured_raw_body.unwrap_or_default();
        let mut content = response.content;
        normalize_synthetic_ids(&mut content, &mut ToolProgress::default());
        response::completion(
            session,
            content,
            response.reasoning_content,
            Some(response.usage),
            response.stop_reason.map(|reason| reason.raw().to_string()),
            response.response_id,
            raw,
        )
        .map_err(|error| at_stage(error, Stage::AssembleCompletion))
    })
    .await
}

pub async fn stream_chat(
    session: &Arc<AiProviderSession>,
    request: &AiChatRequest,
    history: &[AiMessage],
    tx: Sender<AiChatStreamDelta>,
    cancel: &CancellationToken,
) -> Result<AiChatCompletion, AiProviderError> {
    let observer = FrameObserver::new(session.protocol, session.responses);
    let frames = Arc::clone(&observer.state);
    guarded(session, cancel, async {
        if session.ollama_compat {
            return super::ollama_compat::complete(session, request, history, Some(&tx)).await;
        }
        let (chat, mut options) = request::convert(session, request, history)
            .map_err(|error| at_stage(error, Stage::PrepareRequest))?;
        options = options.with_raw_frame_sink(observer);
        let mut stream = session
            .client()
            .await?
            .sdk
            .exec_chat_stream(session.target(true)?, chat, Some(&options))
            .await
            .map_err(|error| at_stage(sdk_error(error), Stage::OpenStream))?
            .stream;
        let mut tools = ToolProgress::default();
        while let Some(event) = stream.next().await {
            let generated = frames
                .lock()
                .map_err(|_| AiProviderError::new(Kind::Protocol))?
                .generated;
            let event = event.map_err(|error| {
                let mut error = sdk_error(error);
                error.generated_content = generated;
                at_stage(error, Stage::DecodeStream)
            })?;
            if frames
                .lock()
                .map_err(|_| AiProviderError::new(Kind::Protocol))?
                .failed
            {
                return Err(at_stage(
                    protocol_error(Code::InvalidStreamFrame),
                    Stage::DecodeStream,
                ));
            }
            match event {
                ChatStreamEvent::Chunk(chunk) => {
                    send_delta(
                        &tx,
                        AiChatStreamDelta {
                            text_delta: chunk.content,
                            ..Default::default()
                        },
                    )
                    .await?
                }
                ChatStreamEvent::ReasoningChunk(chunk) => {
                    send_delta(
                        &tx,
                        AiChatStreamDelta {
                            reasoning_delta: Some(chunk.content),
                            ..Default::default()
                        },
                    )
                    .await?
                }
                ChatStreamEvent::ToolCallChunk(chunk) => {
                    let index = frames
                        .lock()
                        .map_err(|_| AiProviderError::new(Kind::Protocol))?
                        .indexes
                        .get(&chunk.tool_call.call_id)
                        .copied();
                    let delta = tools
                        .update(chunk.tool_call, index)
                        .map_err(|error| at_stage(error, Stage::ValidateToolStream))?;
                    send_delta(
                        &tx,
                        AiChatStreamDelta {
                            tool_call_deltas: vec![delta],
                            ..Default::default()
                        },
                    )
                    .await?;
                }
                ChatStreamEvent::End(end) => {
                    let raw = {
                        let frames = frames
                            .lock()
                            .map_err(|_| AiProviderError::new(Kind::Protocol))?;
                        if !frames.terminal {
                            return Err(at_stage(
                                AiProviderError::new(Kind::StreamInterrupted),
                                Stage::DecodeStream,
                            ));
                        }
                        frames.data.clone()
                    };
                    let mut content = end.captured_content.unwrap_or_default();
                    normalize_synthetic_ids(&mut content, &mut tools);
                    let completion = response::completion(
                        session,
                        content,
                        end.captured_reasoning_content,
                        end.captured_usage,
                        end.captured_stop_reason
                            .map(|reason| reason.raw().to_string()),
                        end.captured_response_id,
                        raw,
                    )
                    .map_err(|error| at_stage(error, Stage::AssembleCompletion))?;
                    // End may be the first event exposing a complete tool call.
                    // Check emitted fragments against it before exposing a result.
                    tools
                        .validate(&completion)
                        .map_err(|error| at_stage(error, Stage::ValidateToolStream))?;
                    for call in &completion.tool_calls {
                        let id = call
                            .id
                            .as_deref()
                            .ok_or_else(|| AiProviderError::new(Kind::Protocol))?;
                        if !tools.calls.contains_key(id) {
                            let delta = tools
                                .update_normalized(
                                    genai::chat::ToolCall {
                                        call_id: id.into(),
                                        fn_name: call.name.clone(),
                                        fn_arguments: call.arguments.clone(),
                                        thought_signatures: None,
                                    },
                                    None,
                                )
                                .map_err(|error| at_stage(error, Stage::ValidateToolStream))?;
                            send_delta(
                                &tx,
                                AiChatStreamDelta {
                                    tool_call_deltas: vec![delta],
                                    ..Default::default()
                                },
                            )
                            .await?;
                        }
                    }
                    send_delta(
                        &tx,
                        AiChatStreamDelta {
                            done: true,
                            metadata: Some(completion.metadata.clone()),
                            ..Default::default()
                        },
                    )
                    .await?;
                    return Ok(completion);
                }
                ChatStreamEvent::Start
                | ChatStreamEvent::Heartbeat
                | ChatStreamEvent::ThoughtSignatureChunk(_) => {}
            }
        }
        Err(at_stage(
            AiProviderError::new(Kind::StreamInterrupted),
            Stage::DecodeStream,
        ))
    })
    .await
    .map_err(|mut error| {
        if !session.ollama_compat
            && let Ok(frames) = frames.lock()
        {
            error.generated_content |= frames.generated;
            error.stream = Some(AiStreamDiagnostic {
                frames: frames.frame_count,
                terminal: frames.terminal,
                failed: frames.failed,
                tool_count: frames.indexes.len(),
                failure: frames.failure,
            });
        }
        error
    })
}

async fn guarded<T>(
    session: &AiProviderSession,
    cancel: &CancellationToken,
    future: impl std::future::Future<Output = Result<T, AiProviderError>>,
) -> Result<T, AiProviderError> {
    let result = tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(AiProviderError::new(Kind::Cancelled)),
        result = tokio::time::timeout(Duration::from_millis(session.settings.timeout_ms), future) => result.unwrap_or_else(|_| Err(AiProviderError::new(Kind::Timeout))),
    };
    result.map_err(|mut error| {
        error.protocol = Some(session.protocol);
        error.responses = session.responses;
        error
    })
}

pub(super) async fn send_delta(
    tx: &Sender<AiChatStreamDelta>,
    mut delta: AiChatStreamDelta,
) -> Result<(), AiProviderError> {
    const MAX_CHUNK: usize = 16 * 1024;
    for reasoning in [false, true] {
        let mut text = if reasoning {
            delta.reasoning_delta.take().unwrap_or_default()
        } else {
            std::mem::take(&mut delta.text_delta)
        };
        while !text.is_empty() {
            let mut end = text.len().min(MAX_CHUNK);
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            let rest = text.split_off(end);
            let part = if reasoning {
                AiChatStreamDelta {
                    reasoning_delta: Some(text),
                    ..Default::default()
                }
            } else {
                AiChatStreamDelta {
                    text_delta: text,
                    ..Default::default()
                }
            };
            tx.send(part)
                .await
                .map_err(|_| AiProviderError::new(Kind::Cancelled))?;
            text = rest;
        }
    }
    for mut call in std::mem::take(&mut delta.tool_call_deltas) {
        loop {
            let mut end = call.arguments_delta.len().min(MAX_CHUNK);
            while !call.arguments_delta.is_char_boundary(end) {
                end -= 1;
            }
            let rest = call.arguments_delta.split_off(end);
            let index = call.index;
            tx.send(AiChatStreamDelta {
                tool_call_deltas: vec![call],
                ..Default::default()
            })
            .await
            .map_err(|_| AiProviderError::new(Kind::Cancelled))?;
            if rest.is_empty() {
                break;
            }
            call = AiToolCallDelta {
                index,
                arguments_delta: rest,
                ..Default::default()
            };
        }
    }
    if delta.done || delta.metadata.is_some() {
        tx.send(delta)
            .await
            .map_err(|_| AiProviderError::new(Kind::Cancelled))?;
    }
    Ok(())
}

#[derive(Default)]
struct ToolProgress {
    calls: BTreeMap<String, (usize, String, String)>,
    ids: BTreeMap<String, String>,
}

impl ToolProgress {
    fn id(&mut self, id: &str) -> String {
        self.ids
            .entry(id.into())
            .or_insert_with(|| {
                // Gemini synthesizes the same call#name#0 again on the next turn.
                // Keep its recognizable shape, with a process-unique numeric suffix.
                if let Some((prefix, _)) = id.rsplit_once('#')
                    && id.starts_with("call#")
                {
                    static NEXT_CALL: std::sync::atomic::AtomicU64 =
                        std::sync::atomic::AtomicU64::new(1);
                    format!(
                        "{prefix}#{}",
                        NEXT_CALL.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                    )
                } else {
                    id.to_string()
                }
            })
            .clone()
    }

    fn update(
        &mut self,
        mut call: genai::chat::ToolCall,
        wire_index: Option<usize>,
    ) -> Result<AiToolCallDelta, AiProviderError> {
        call.call_id = self.id(&call.call_id);
        self.update_normalized(call, wire_index)
    }

    // The final completion already uses domain IDs. Never feed them back into
    // the raw-ID map: a mapped ID can equal another SDK-generated raw ID.
    fn update_normalized(
        &mut self,
        call: genai::chat::ToolCall,
        wire_index: Option<usize>,
    ) -> Result<AiToolCallDelta, AiProviderError> {
        let id = call.call_id;
        if !self.calls.contains_key(&id) && self.calls.len() >= 128 {
            return Err(protocol_error(Code::ToolCallLimit));
        }
        let next_index = wire_index.unwrap_or_else(|| {
            self.calls
                .values()
                .map(|(index, _, _)| index.saturating_add(1))
                .max()
                .unwrap_or_default()
        });
        let first = !self.calls.contains_key(&id);
        if let Some(index) = wire_index
            && self.calls.iter().any(|(other, (slot, _, _))| {
                (other != &id && *slot == index) || (other == &id && *slot != index)
            })
        {
            return Err(protocol_error(Code::ToolIndexConflict));
        }
        let existing = self
            .calls
            .entry(id.clone())
            .or_insert_with(|| (next_index, String::new(), String::new()));
        let arguments = match call.fn_arguments {
            serde_json::Value::String(value) => value,
            serde_json::Value::Null => String::new(),
            value => value.to_string(),
        };
        let suffix = arguments
            .strip_prefix(&existing.2)
            .ok_or_else(|| protocol_error(Code::ToolArgumentsChanged))?
            .to_string();
        let name = if existing.1.is_empty() {
            Some(call.fn_name.clone()).filter(|name| !name.is_empty())
        } else if !call.fn_name.is_empty() && call.fn_name != existing.1 {
            return Err(protocol_error(Code::ToolNameChanged));
        } else {
            None
        };
        if let Some(name) = &name {
            existing.1 = name.clone();
        }
        existing.2 = arguments;
        Ok(AiToolCallDelta {
            index: existing.0,
            id_delta: first.then_some(id),
            name_delta: name,
            arguments_delta: suffix,
            thought_signature: call
                .thought_signatures
                .and_then(|signatures| signatures.into_iter().next()),
        })
    }

    fn validate(&self, completion: &AiChatCompletion) -> Result<(), AiProviderError> {
        for (id, (_, name, arguments)) in &self.calls {
            let call = completion
                .tool_calls
                .iter()
                .find(|call| call.id.as_deref() == Some(id))
                .ok_or_else(|| protocol_error(Code::ToolResultMismatch))?;
            if (!name.is_empty() && name != &call.name)
                || (!arguments.is_empty()
                    && serde_json::from_str::<serde_json::Value>(arguments)
                        .ok()
                        .as_ref()
                        != Some(&call.arguments))
            {
                return Err(protocol_error(Code::ToolResultMismatch));
            }
        }
        Ok(())
    }
}

fn normalize_synthetic_ids(content: &mut MessageContent, tools: &mut ToolProgress) {
    let parts = std::mem::take(content)
        .into_parts()
        .into_iter()
        .map(|part| match part {
            ContentPart::ToolCall(mut call) => {
                call.call_id = tools.id(&call.call_id);
                ContentPart::ToolCall(call)
            }
            part => part,
        })
        .collect::<Vec<_>>();
    *content = MessageContent::from_parts(parts);
}

#[cfg(test)]
mod tests {
    use super::ToolProgress;

    #[test]
    fn raw_synthetic_id_does_not_alias_an_already_normalized_call() {
        let mut progress = ToolProgress::default();
        let first = progress.id("call#session_get#0");
        // Feed a second raw ID with exactly the first call's normalized value.
        // SDK numbering restarts per turn, independently of our global counter.
        let second = progress.id(&first);
        assert_ne!(first, second);
        assert_eq!(progress.id("call#session_get#0"), first);
        assert_eq!(progress.id(&first), second);
    }
}
