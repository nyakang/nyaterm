use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures::channel::mpsc::UnboundedSender;
use nyaterm_agent_client::acp::{AgentEvent, AgentOperation};
use nyaterm_core::ai::external_agents::{
    ExternalAgentProfile, profiles, resolve_profile, same_agent,
};
use nyaterm_core::{
    AiChatRequest, AiHistoryFile, AiMessage, AiMessageRole, AiMode, AiSettings, now_rfc3339, uuid,
};
use nyaterm_store::{StoreBlockingClient, StoreDomain};

use crate::features::ai::helper_resolver::resolve_mcp_helper;
use crate::features::mcp::McpEphemeralCredential;
use crate::features::runtime_jobs::{AiChatJobOutput, AiChatWorkerEvent};

/// Runs on the caller's worker. Resolved secrets never travel back to UI state.
pub(super) fn prepare_profile(
    store: &StoreBlockingClient,
    mut profile: ExternalAgentProfile,
) -> Result<ExternalAgentProfile, String> {
    if !profile
        .environment
        .iter()
        .any(|variable| variable.value == nyaterm_core::MASKED_SECRET_VALUE)
    {
        return Ok(profile);
    }
    store
        .request_fn(StoreDomain::Ai, move |database| {
            let current = database.load_ai_settings()?;
            let previous = profiles(&current).iter().find(|old| old.id == profile.id);
            profile.merge_masked_environment(previous);
            Ok(profile)
        })
        .map_err(|_| "Cannot load Agent credentials".to_owned())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run_external(
    store: &StoreBlockingClient,
    settings: &AiSettings,
    request: &AiChatRequest,
    history: &AiHistoryFile,
    credential: McpEphemeralCredential,
    stream: Option<&UnboundedSender<AiChatWorkerEvent>>,
    cancel: &Arc<AtomicBool>,
    job_id: u64,
) -> Result<AiChatJobOutput, String> {
    let profile = prepare_profile(
        store,
        resolve_profile(settings, &request.agent_kind)
            .ok_or("Agent configuration no longer exists")?
            .clone(),
    )?;
    if !profile.enabled {
        return Err("Agent is disabled".into());
    }
    let session_id = request
        .session_id
        .as_deref()
        .ok_or("AI session is missing")?;
    let previous = history
        .sessions
        .iter()
        .find(|session| session.id == session_id);
    if previous.is_some_and(|session| !same_agent(&session.agent_kind, &request.agent_kind)) {
        return Err("AI session belongs to another Agent instance".into());
    }
    let binding = profile
        .persistent
        .then(|| {
            previous
                .and_then(|session| session.backend_metadata.as_ref())
                .and_then(|metadata| metadata.external_binding.clone())
        })
        .flatten();
    let legacy_session_id = (profile.persistent && binding.is_none())
        .then(|| {
            previous.and_then(|session| {
                session.external_session_id.clone().or_else(|| {
                    session
                        .backend_metadata
                        .as_ref()
                        .and_then(|metadata| metadata.external_thread_id.clone())
                })
            })
        })
        .flatten();
    if profile.persistent
        && previous.is_some()
        && binding.is_none()
        && (legacy_session_id.is_none() || profile.revision != 0)
    {
        return Err("This saved conversation cannot be restored with the current Agent configuration. Use New chat from history to review and reuse its text.".into());
    }
    let helper = resolve_mcp_helper()?;
    let kind = profile.agent_kind();
    let started = std::time::Instant::now();
    let result = nyaterm_agent_client::acp::run(
        &profile,
        AgentOperation::Prompt {
            text: nyaterm_core::ai::build_agent_prompt(request, settings),
            model: request.model_name.clone(),
            binding: binding.map(Box::new),
            legacy_session_id,
        },
        Some(&helper),
        credential.environment(),
        cancel,
        Duration::from_millis(settings.timeout_ms.max(1_000)),
        &mut |event| {
            if cancel.load(Ordering::Acquire) {
                return Ok(());
            }
            match event {
                AgentEvent::Delta { text, reasoning } => {
                    if let Some(stream) = stream {
                        stream
                            .unbounded_send(AiChatWorkerEvent::Delta {
                                job_id,
                                session_id: session_id.to_owned(),
                                text_delta: text,
                                reasoning_delta: reasoning,
                            })
                            .map_err(|_| "AI event receiver closed".to_owned())?;
                    }
                }
                AgentEvent::Session(binding) if settings.record_history && profile.persistent => {
                    let session_id = session_id.to_owned();
                    let kind = kind.clone();
                    let cancel = cancel.clone();
                    let mut metadata = previous
                        .and_then(|session| session.backend_metadata.clone())
                        .unwrap_or_default();
                    metadata.backend = if profile.strategy.0 == "claude" {
                        nyaterm_core::AiBackendKind::ClaudeCode
                    } else {
                        nyaterm_core::AiBackendKind::Codex
                    };
                    metadata.external_binding = Some(binding.clone());
                    store
                        .request_fn(StoreDomain::Ai, move |database| {
                            if cancel.load(Ordering::Acquire) {
                                return Ok(());
                            }
                            database.set_ai_session_external_metadata(
                                &session_id,
                                kind,
                                binding.external_id.clone(),
                                metadata,
                            )
                        })
                        .map_err(|error| error.to_string())?;
                }
                AgentEvent::Tool { .. } => {
                    if let Some(stream) = stream {
                        stream
                            .unbounded_send(AiChatWorkerEvent::AgentToolCallDelta {
                                job_id,
                                session_id: session_id.to_owned(),
                                tool_name: None,
                                arguments_delta_len: 0,
                            })
                            .map_err(|_| "AI event receiver closed".to_owned())?;
                    }
                }
                _ => {}
            }
            Ok(())
        },
        &mut || credential.revoke(),
    )
    .map_err(|error| error.to_string());
    drop(credential);
    super::ai_jobs::record_external_agent_audit(
        store,
        request,
        "acp",
        "completed",
        result.is_ok(),
        result.as_ref().err().map(String::as_str),
        Some(started.elapsed()),
    );
    let completion = result?;
    if cancel.load(Ordering::Acquire) {
        return Err("AI request cancelled".into());
    }
    if settings.record_history {
        let message = AiMessage {
            id: format!("msg-{}", uuid()),
            session_id: session_id.into(),
            role: AiMessageRole::Assistant,
            content: completion.text.clone(),
            created_at: now_rfc3339(),
            reasoning_content: completion.reasoning.clone(),
            command_cards: Vec::new(),
        };
        let cancel = cancel.clone();
        store
            .request_fn(StoreDomain::Ai, move |database| {
                if cancel.load(Ordering::Acquire) {
                    return Ok(());
                }
                database.append_ai_message(message)
            })
            .map_err(|error| error.to_string())?;
    }
    Ok(AiChatJobOutput {
        metadata: None,
        native_decision: None,
        mode: AiMode::Agent,
        text: completion.text,
        reasoning: completion.reasoning,
        command_cards: Vec::new(),
        auto_execute_first: false,
        approval_note: None,
    })
}

#[cfg(test)]
mod tests {
    use crate::features::ai::acp_runtime::prepare_profile;
    use crate::test_support::{TestConfigDir, blocking_test_store};
    use nyaterm_core::ai::external_agents::{AgentEnvironmentVariable, ExternalAgentProfile};
    use nyaterm_core::{AiSettings, mask_ai_settings};
    use nyaterm_store::StoreDomain;

    #[test]
    fn worker_resolves_saved_environment_masks_without_exposing_or_crossing_identities() {
        let root = TestConfigDir::new("nyaterm-acp-worker-credentials");
        let store = blocking_test_store(root.path());
        let mut profile = ExternalAgentProfile::new("account".into(), "claude");
        profile.environment.push(AgentEnvironmentVariable {
            id: "key".into(),
            name: "ANTHROPIC_API_KEY".into(),
            value: "fixture-secret".into(),
        });
        let settings = AiSettings {
            external_agents: Some(vec![profile]),
            ..Default::default()
        };
        let masked = store
            .request_fn(StoreDomain::Ai, move |database| {
                database.save_ai_settings(settings).map(mask_ai_settings)
            })
            .unwrap();
        let draft = &masked.external_agents.as_ref().unwrap()[0];
        let resolved = prepare_profile(&store, draft.clone()).unwrap();
        assert_eq!(
            resolved.environment[0].value.expose_secret(),
            "fixture-secret"
        );
        assert_eq!(
            draft.environment[0].value,
            nyaterm_core::MASKED_SECRET_VALUE
        );

        let mut changed = draft.clone();
        changed.environment[0].value = "unsaved-secret".into();
        assert_eq!(prepare_profile(&store, changed.clone()).unwrap(), changed);
        for identity in ["profile", "row", "name"] {
            let mut changed = draft.clone();
            match identity {
                "profile" => changed.id = "another-account".into(),
                "row" => changed.environment[0].id = "replacement".into(),
                _ => changed.environment[0].name = "OTHER_KEY".into(),
            }
            assert!(
                prepare_profile(&store, changed).unwrap().environment[0]
                    .value
                    .is_empty()
            );
        }
    }
}
