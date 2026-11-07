//! Ephemeral model communication contracts. None of these values is persisted.

use std::fmt;
use std::time::Duration;

use serde_json::{Number, Value};

use super::AiProviderApiProtocol;

#[derive(Clone, Default, PartialEq, Eq)]
pub struct AiGenerationOptions {
    pub temperature: Option<Number>,
    pub max_tokens: Option<u32>,
    pub thinking_budget: Option<u32>,
    pub extra_body: Option<Value>,
}

impl fmt::Debug for AiGenerationOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AiGenerationOptions")
            .field("max_tokens", &self.max_tokens)
            .field("thinking_budget", &self.thinking_budget)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AiUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub cached_tokens: Option<u64>,
    pub cache_creation_tokens: Option<u64>,
}

/// Provider-native continuation data is deliberately opaque to the harness.
/// The adapter owns interpretation; the harness only associates it with a turn.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct AiAssistantTurn {
    pub protocol: Option<AiProviderApiProtocol>,
    pub responses: bool,
    pub parts: Vec<AiContentPart>,
    /// Original ordered output items/parts, including unknown provider fields.
    pub provider_items: Vec<Value>,
}

#[derive(Clone, PartialEq, Eq)]
pub enum AiContentPart {
    Text(String),
    Reasoning(String),
    Signature(String),
    ToolCall {
        id: String,
        name: String,
        arguments: Value,
        signatures: Vec<String>,
    },
    /// Includes the native data of SDK custom/binary parts without SDK types.
    Custom(Value),
    Binary(Value),
}

impl fmt::Debug for AiAssistantTurn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AiAssistantTurn")
            .field("protocol", &self.protocol)
            .field("responses", &self.responses)
            .field("part_count", &self.parts.len())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct AiResponseMetadata {
    pub usage: AiUsage,
    pub finish_reason: Option<String>,
    pub response_id: Option<String>,
    pub provider_model: Option<String>,
    pub assistant_turn: AiAssistantTurn,
    /// Retains provider-specific usage and response fields, never logged/saved.
    pub provider_data: Value,
}

impl fmt::Debug for AiResponseMetadata {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AiResponseMetadata")
            .field("usage", &self.usage)
            .field("assistant_turn", &self.assistant_turn)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiProviderErrorKind {
    Authentication,
    InvalidApiKey,
    InvalidModel,
    InvalidRequest,
    RateLimit,
    Timeout,
    Connection,
    Tls,
    Server,
    StreamInterrupted,
    Protocol,
    Cancelled,
    UnsupportedCapability,
}

/// Allowlisted classifications; provider prose and arbitrary codes never cross the boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiProviderDiagnosticCode {
    InvalidRequest,
    UnsupportedParameter,
    UnsupportedTools,
    ContextLength,
    InvalidSignature,
    InvalidToolResults,
    SdkStreamParse,
    SdkResponseGeneration,
    SdkResponseShape,
    ProviderStreamError,
    SdkOther,
    InvalidStreamFrame,
    ToolCallLimit,
    ToolIndexConflict,
    ToolArgumentsChanged,
    ToolNameChanged,
    ToolResultMismatch,
    InvalidCompletion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiProviderFailureStage {
    PrepareRequest,
    OpenStream,
    ReadResponse,
    DecodeStream,
    AssembleCompletion,
    ValidateToolStream,
}

/// Counts and flags only; never provider text, IDs, arguments, or signatures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AiStreamDiagnostic {
    pub frames: usize,
    pub terminal: bool,
    pub failed: bool,
    pub tool_count: usize,
    pub failure: Option<AiStreamFrameFailure>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiStreamFrameFailureReason {
    InvalidJson,
    InvalidEnvelope,
    ProviderError,
    ContinuationSizeLimit,
    MissingResponseObject,
    MissingOutputItemObject,
    InvalidContentBlock,
    DuplicateContentBlock,
    MissingContentBlock,
    InvalidToolArguments,
    UnclosedToolArguments,
    StructureLimit,
}

/// Provider event names are mapped to these fixed categories before diagnostics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AiStreamEventKind {
    Lifecycle,
    OutputItemAdded,
    OutputItemDone,
    FunctionArguments,
    Text,
    Reasoning,
    Completed,
    Incomplete,
    Failed,
    Error,
    #[default]
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AiStreamFrameFailure {
    pub reason: AiStreamFrameFailureReason,
    /// One-based received frame number, frozen at the first failure.
    pub frame: usize,
    pub event: AiStreamEventKind,
    pub data_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiProviderParameter {
    Tools,
    ToolChoice,
    ParallelToolCalls,
    Reasoning,
    Temperature,
    MaxTokens,
    Messages,
    Input,
}

/// Only machine classifications and safe context cross the HTTP boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiProviderError {
    pub kind: AiProviderErrorKind,
    pub status: Option<u16>,
    pub retry_after: Option<Duration>,
    pub retryable: bool,
    pub protocol: Option<AiProviderApiProtocol>,
    pub generated_content: bool,
    pub responses: bool,
    pub tool_protocol_unsupported: bool,
    pub diagnostic_code: Option<AiProviderDiagnosticCode>,
    pub parameter: Option<AiProviderParameter>,
    pub request_id: Option<String>,
    pub failure_stage: Option<AiProviderFailureStage>,
    pub stream: Option<AiStreamDiagnostic>,
}

impl AiProviderError {
    pub fn new(kind: AiProviderErrorKind) -> Self {
        Self {
            kind,
            status: None,
            retry_after: None,
            retryable: matches!(
                kind,
                AiProviderErrorKind::RateLimit
                    | AiProviderErrorKind::Timeout
                    | AiProviderErrorKind::Connection
                    | AiProviderErrorKind::Server
            ),
            protocol: None,
            generated_content: false,
            responses: false,
            tool_protocol_unsupported: false,
            diagnostic_code: None,
            parameter: None,
            request_id: None,
            failure_stage: None,
            stream: None,
        }
    }
}

impl fmt::Display for AiProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self.kind {
            AiProviderErrorKind::Authentication => "AI provider authentication failed",
            AiProviderErrorKind::InvalidApiKey => "AI provider rejected the API key",
            AiProviderErrorKind::InvalidModel => "AI provider rejected the model",
            AiProviderErrorKind::InvalidRequest => "AI provider rejected the request",
            AiProviderErrorKind::RateLimit => "AI provider rate limit reached",
            AiProviderErrorKind::Timeout => "AI request timed out",
            AiProviderErrorKind::Connection => "Cannot connect to AI provider",
            AiProviderErrorKind::Tls => "AI provider TLS verification failed",
            AiProviderErrorKind::Server => "AI provider server error",
            AiProviderErrorKind::StreamInterrupted => "AI stream ended before completion",
            AiProviderErrorKind::Protocol => "Invalid AI provider response",
            AiProviderErrorKind::Cancelled => "AI request cancelled",
            AiProviderErrorKind::UnsupportedCapability => {
                "AI provider does not support this request capability"
            }
        };
        f.write_str(message)?;
        if let Some(status) = self.status {
            write!(f, " (HTTP {status})")?;
        }
        if let Some(code) = self.diagnostic_code {
            write!(f, " [{code:?}]")?;
        }
        if let Some(stage) = self.failure_stage {
            write!(f, " [stage: {stage:?}]")?;
        }
        if let Some(failure) = self.stream.and_then(|stream| stream.failure) {
            write!(
                f,
                " [frame {}: {:?}, {:?}]",
                failure.frame, failure.reason, failure.event
            )?;
        }
        if let Some(parameter) = self.parameter {
            write!(f, " ({parameter:?})")?;
        }
        if let Some(id) = &self.request_id {
            write!(f, " [request: {id}]")?;
        }
        Ok(())
    }
}

impl std::error::Error for AiProviderError {}

/// Business prompt/history selection, independent of every provider's wire format.
pub fn request_messages(
    request: &super::AiChatRequest,
    settings: &super::AiSettings,
    history: &[super::AiMessage],
) -> Vec<(super::AiMessageRole, String)> {
    use super::AiMessageRole;
    let mut system = super::request_system_prompt(request).to_string();
    system.push_str(&super::prompts::run_guidance(request));
    let mut messages = vec![(AiMessageRole::System, system)];
    if !request.options.connectivity_test
        && let Some(session) = &request.session_id
    {
        let turns: Vec<_> = history
            .iter()
            .filter(|message| &message.session_id == session)
            .collect();
        let skip = turns
            .len()
            .saturating_sub(request.options.history_turns as usize);
        for message in turns.into_iter().skip(skip) {
            match message.role {
                AiMessageRole::User => {
                    messages.push((AiMessageRole::User, message.content.clone()))
                }
                AiMessageRole::Assistant => {
                    let text = super::extract_text_from_assistant(&message.content);
                    if !text.is_empty() {
                        messages.push((AiMessageRole::Assistant, text));
                    }
                }
                AiMessageRole::System => {}
            }
        }
    }
    messages.push((
        AiMessageRole::User,
        super::request_user_prompt(request, settings),
    ));
    messages
}

#[cfg(test)]
mod tests {
    use super::{AiAssistantTurn, AiContentPart, AiGenerationOptions};
    use crate::ai::harness::AgentToolRegistry;
    use crate::ai::{AiRequestOptions, AiToolCall};
    use serde_json::json;

    #[test]
    fn transient_generation_and_signed_turns_never_enter_saved_contracts() {
        let old =
            json!({"maxOutputCommands":3,"language":"zh","safetyMode":"strict","historyTurns":12});
        let mut options: AiRequestOptions = serde_json::from_value(old.clone()).unwrap();
        options.generation = AiGenerationOptions {
            extra_body: Some(json!({"secret":"private-fixture"})),
            ..Default::default()
        };
        options.agent_protocol_repair = true;
        assert_eq!(serde_json::to_value(&options).unwrap(), old);
        assert!(
            !serde_json::from_value::<AiRequestOptions>(old)
                .unwrap()
                .agent_protocol_repair
        );
        assert!(!format!("{options:?}").contains("private-fixture"));
        let mut call = AgentToolRegistry::parse(
            &[AiToolCall {
                id: Some("a".into()),
                name: "final_answer".into(),
                arguments: json!({"answer":"ok"}),
                thought_signature: None,
            }],
            "",
        )
        .unwrap();
        let before = serde_json::to_value(&call).unwrap();
        call.assistant_turn = Some(AiAssistantTurn {
            parts: vec![AiContentPart::Signature("private-fixture".into())],
            provider_items: vec![json!({"encrypted_content":"private-fixture"})],
            ..Default::default()
        });
        assert_eq!(serde_json::to_value(&call).unwrap(), before);
        assert!(!format!("{call:?}").contains("private-fixture"));
    }
}
