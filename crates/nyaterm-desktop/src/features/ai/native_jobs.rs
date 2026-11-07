use std::sync::Arc;

use gpui::Context;
use nyaterm_core::ai::completion::AiProviderErrorKind;
use nyaterm_core::ai::harness::AgentToolRegistry;
use nyaterm_core::ai::{AiChatCompletion, AiChatRequest, AiChatStreamDelta, AiMessage, AiMode};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::app_shell::DesktopControllerGlobal;
use crate::blocking_jobs::BlockingJobScheduler;
use crate::features::NyaTermApp;
use crate::features::runtime_jobs::{AiChatJobOutput, AiChatJobResult, AiChatWorkerEvent};
use crate::http::ai::{AiProviderSession, stream_chat};
use nyaterm_store::StoreBlockingClient;

use super::ai_jobs::{finish_native_job, prepare_native_job};
use super::state::AiChatLaunch;

// One queue orders attempt boundaries after all preceding deltas and before
// any new response. Separate notification channels can race queued text.
enum NativeJobEvent {
    Delta(Box<AiChatStreamDelta>),
    ProtocolFallback,
    ProtocolRepair,
    Finished(Box<Result<AiChatJobOutput, String>>),
}

impl NyaTermApp {
    pub(in crate::features) fn launch_native_chat(
        &mut self,
        request: AiChatRequest,
        launch: AiChatLaunch,
        history: Option<Vec<AiMessage>>,
        persist: bool,
        cx: &mut Context<Self>,
    ) {
        let job_id = launch.job_id;
        let session_id = launch.session_id;
        let runtime = cx
            .try_global::<DesktopControllerGlobal>()
            .ok_or_else(|| "AI runtime unavailable".to_string())
            .and_then(|global| global.0.read(cx).ai_runtime.clone());
        let session = self.ai.provider_session(&request);
        let (runtime, session) =
            match runtime.and_then(|runtime| session.map(|session| (runtime, session))) {
                Ok(value) => value,
                Err(error) => {
                    self.dispatch_ai_chat_event(
                        AiChatWorkerEvent::Finished(Box::new(AiChatJobResult {
                            job_id,
                            session_id,
                            result: Err(error),
                        })),
                        cx,
                    );
                    return;
                }
            };
        let cancel = self.ai.native_network_cancel();
        let jobs = self.blocking_jobs.clone();
        let store = self.store_blocking_client();
        let (tx, mut rx) = mpsc::channel(64);
        let task_cancel = cancel.clone();
        let run_id = request
            .options
            .agent_context
            .as_ref()
            .map(|context| context.run_id.clone());
        let task = runtime.spawn(cancel, async move {
            let result = run_native_job(
                jobs,
                store,
                session,
                request,
                history,
                persist,
                &tx,
                &task_cancel,
            )
            .await;
            let _ = tx.send(NativeJobEvent::Finished(Box::new(result))).await;
        });
        self.ai.own_native_task(task);
        cx.spawn(async move |this, cx| {
            while let Some(event) = rx.recv().await {
                let result = this.update(cx, |this, cx| {
                    if run_id
                        .as_ref()
                        .is_some_and(|run| !this.ai.native_run_is_live(run, &session_id))
                    {
                        return;
                    }
                    match event {
                        NativeJobEvent::Delta(delta) => {
                            if !delta.text_delta.is_empty() || delta.reasoning_delta.is_some() {
                                this.dispatch_ai_chat_event(
                                    AiChatWorkerEvent::Delta {
                                        job_id,
                                        session_id: session_id.clone(),
                                        text_delta: delta.text_delta,
                                        reasoning_delta: delta.reasoning_delta,
                                    },
                                    cx,
                                );
                            }
                            for tool in delta.tool_call_deltas {
                                this.dispatch_ai_chat_event(
                                    AiChatWorkerEvent::AgentToolCallDelta {
                                        job_id,
                                        session_id: session_id.clone(),
                                        tool_name: tool.name_delta,
                                        arguments_delta_len: tool.arguments_delta.len(),
                                    },
                                    cx,
                                );
                            }
                        }
                        NativeJobEvent::ProtocolFallback => this.dispatch_ai_chat_event(
                            AiChatWorkerEvent::AgentProtocolFallback {
                                job_id,
                                session_id: session_id.clone(),
                            },
                            cx,
                        ),
                        NativeJobEvent::ProtocolRepair => this.dispatch_ai_chat_event(
                            AiChatWorkerEvent::AgentProtocolRepair {
                                job_id,
                                session_id: session_id.clone(),
                            },
                            cx,
                        ),
                        NativeJobEvent::Finished(result) => this.dispatch_ai_chat_event(
                            AiChatWorkerEvent::Finished(Box::new(AiChatJobResult {
                                job_id,
                                session_id: session_id.clone(),
                                result: *result,
                            })),
                            cx,
                        ),
                    }
                });
                if result.is_err() {
                    return;
                }
            }
        })
        .detach();
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_native_job(
    jobs: BlockingJobScheduler,
    store: StoreBlockingClient,
    session: Arc<AiProviderSession>,
    mut request: AiChatRequest,
    history: Option<Vec<AiMessage>>,
    persist: bool,
    tx: &mpsc::Sender<NativeJobEvent>,
    cancel: &CancellationToken,
) -> Result<AiChatJobOutput, String> {
    let prep_store = store.clone();
    let prep_session = session.clone();
    let prepare_cancel = cancel.clone();
    let (mut request, history) = jobs
        .submit_task("ai-prepare", move |cancel| {
            if cancel.is_cancelled() || prepare_cancel.is_cancelled() {
                return Err("AI request cancelled".to_string());
            }
            let history = prepare_native_job(
                &prep_store,
                &prep_session.settings,
                &mut request,
                history,
                persist,
            )?;
            Ok((request, history))
        })
        .map_err(|error| error.to_string())?
        .await
        .map_err(|error| error.to_string())??;
    let completion = completion_with_repair(&session, &mut request, &history, tx, cancel).await?;
    if cancel.is_cancelled() {
        return Err("AI request cancelled".into());
    }
    let finish_cancel = cancel.clone();
    jobs.submit_task("ai-finish", move |cancel| {
        if cancel.is_cancelled() || finish_cancel.is_cancelled() {
            return Err("AI request cancelled".into());
        }
        finish_native_job(&store, &session.settings, &request, completion)
    })
    .map_err(|error| error.to_string())?
    .await
    .map_err(|error| error.to_string())?
}

async fn completion_with_repair(
    session: &Arc<AiProviderSession>,
    request: &mut AiChatRequest,
    history: &[AiMessage],
    tx: &mpsc::Sender<NativeJobEvent>,
    cancel: &CancellationToken,
) -> Result<AiChatCompletion, String> {
    // Protocol fallback and response correction share one extra model POST.
    // Only model responses are retried; no tool has been dispatched here.
    for attempt in 1..=2 {
        let result = stream_attempt(session, request, history, tx, cancel).await;
        if let Err(error) = &result
            && error.kind != AiProviderErrorKind::Cancelled
        {
            warn_native_request_failure(error, request, attempt);
        }
        let completion = match result {
            Err(error)
                if attempt == 1
                    && error.kind == AiProviderErrorKind::UnsupportedCapability
                    && error.tool_protocol_unsupported
                    && !error.generated_content
                    && request.mode == AiMode::Agent
                    && !request.options.agent_json_protocol
                    && request
                        .options
                        .agent_context
                        .as_ref()
                        .is_none_or(|context| context.turns.is_empty()) =>
            {
                request.options.agent_json_protocol = true;
                send_event(tx, NativeJobEvent::ProtocolFallback, cancel).await?;
                continue;
            }
            result => result.map_err(|error| error.to_string())?,
        };
        if cancel.is_cancelled() {
            return Err("AI request cancelled".into());
        }
        if request.mode == AiMode::Agent
            && request.options.agent_context.is_some()
            && let Err(error) = AgentToolRegistry::decision(&completion)
        {
            warn_agent_protocol_failure(&completion, attempt, &error);
            if attempt == 1
                && error == nyaterm_core::ai::harness::AgentResponseError::InvalidJsonAction
            {
                request.options.agent_protocol_repair = true;
                send_event(tx, NativeJobEvent::ProtocolRepair, cancel).await?;
                continue;
            }
            return Err(error.to_string());
        }
        return Ok(completion);
    }
    unreachable!("the second attempt always returns a result")
}

fn warn_native_request_failure(
    error: &nyaterm_core::ai::completion::AiProviderError,
    request: &AiChatRequest,
    attempt: usize,
) {
    // Deliberate allowlist: do not log the SDK error, request, model name,
    // provider URL, raw frames, arguments, or any conversation content.
    let turns = request
        .options
        .agent_context
        .as_ref()
        .map(|context| &context.turns);
    tracing::warn!(
        kind = ?error.kind,
        classification = ?error.diagnostic_code,
        stage = ?error.failure_stage,
        protocol = ?error.protocol,
        responses = error.responses,
        status = ?error.status,
        parameter = ?error.parameter,
        request_id = error.request_id.as_deref(),
        attempt,
        json_protocol = request.options.agent_json_protocol,
        completed_turns = turns.map_or(0, Vec::len),
        completed_calls = turns.map_or(0, |turns| turns.iter().map(|turn| turn.results.len()).sum::<usize>()),
        generated_content = error.generated_content,
        stream = ?error.stream,
        "AI native request failed"
    );
}

fn warn_agent_protocol_failure(
    completion: &AiChatCompletion,
    attempt: usize,
    error: &nyaterm_core::ai::harness::AgentResponseError,
) {
    let classification = match error {
        nyaterm_core::ai::harness::AgentResponseError::InvalidJsonAction => "malformed_json_action",
        nyaterm_core::ai::harness::AgentResponseError::InvalidStructure => "invalid_call_structure",
        nyaterm_core::ai::harness::AgentResponseError::InvalidArguments => "invalid_tool_arguments",
        nyaterm_core::ai::harness::AgentResponseError::Empty => "empty_answer",
        nyaterm_core::ai::harness::AgentResponseError::ReasoningOnly => "reasoning_only",
        nyaterm_core::ai::harness::AgentResponseError::Refused => "refused",
        nyaterm_core::ai::harness::AgentResponseError::Truncated => "truncated",
    };
    tracing::warn!(
        classification,
        attempt,
        protocol = ?completion.metadata.assistant_turn.protocol,
        responses = completion.metadata.assistant_turn.responses,
        tool_count = completion.tool_calls.len(),
        finish_reason = completion.metadata.finish_reason.as_deref().filter(|value| matches!(*value, "stop" | "end_turn" | "tool_calls" | "tool_use" | "length" | "max_tokens" | "refusal")).unwrap_or("other"),
        text_empty = completion.text.is_empty(),
        text_chars = completion.text.chars().count(),
        "AI Agent response failed protocol validation"
    );
}

async fn send_event(
    tx: &mpsc::Sender<NativeJobEvent>,
    event: NativeJobEvent,
    cancel: &CancellationToken,
) -> Result<(), String> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err("AI request cancelled".into()),
        result = tx.send(event) => result.map_err(|_| "AI response consumer closed".into()),
    }
}

async fn stream_attempt(
    session: &Arc<AiProviderSession>,
    request: &AiChatRequest,
    history: &[AiMessage],
    tx: &mpsc::Sender<NativeJobEvent>,
    cancel: &CancellationToken,
) -> Result<AiChatCompletion, nyaterm_core::ai::completion::AiProviderError> {
    use nyaterm_core::ai::completion::AiProviderError;

    let (delta_tx, mut delta_rx) = mpsc::channel(64);
    let response = stream_chat(session, request, history, delta_tx, cancel);
    tokio::pin!(response);
    let mut deltas_open = true;
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(AiProviderError::new(AiProviderErrorKind::Cancelled)),
            delta = delta_rx.recv(), if deltas_open => {
                if let Some(delta) = delta {
                    send_event(tx, NativeJobEvent::Delta(Box::new(delta)), cancel).await
                        .map_err(|_| AiProviderError::new(AiProviderErrorKind::Cancelled))?;
                } else {
                    deltas_open = false;
                }
            }
            result = &mut response => {
                // The SDK has finished sending, but its bounded queue may still
                // contain deltas. Drain them before exposing the attempt boundary.
                while let Ok(delta) = delta_rx.try_recv() {
                    send_event(tx, NativeJobEvent::Delta(Box::new(delta)), cancel).await
                        .map_err(|_| AiProviderError::new(AiProviderErrorKind::Cancelled))?;
                }
                return result;
            }
        }
    }
}

#[cfg(test)]
mod tests;
