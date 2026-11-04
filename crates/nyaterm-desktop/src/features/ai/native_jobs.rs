use std::sync::Arc;

use gpui::Context;
use nyaterm_core::ai::completion::AiProviderErrorKind;
use nyaterm_core::ai::{AiChatRequest, AiMessage};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::app_shell::DesktopControllerGlobal;
use crate::blocking_jobs::BlockingJobScheduler;
use crate::features::NyaTermApp;
use crate::features::runtime_jobs::{AiChatJobOutput, AiChatJobResult, AiChatWorkerEvent};
use crate::http::ai::{AiProviderSession, stream_chat};
use nyaterm_store::StoreBlockingClient;

use super::ai_jobs::{finish_native_job, prepare_native_job};
use super::state::AiChatLaunch;

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
        let (result_tx, result_rx) = oneshot::channel();
        let (fallback_tx, mut fallback_rx) = oneshot::channel();
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
                tx,
                fallback_tx,
                &task_cancel,
            )
            .await;
            let _ = result_tx.send(result);
        });
        self.ai.own_native_task(task);
        cx.spawn(async move |this, cx| {
            let mut fallback_pending = true;
            loop {
                let delta = tokio::select! {
                    biased;
                    fallback = &mut fallback_rx, if fallback_pending => {
                        fallback_pending = false;
                        if fallback.is_ok() && this.update(cx, |this, cx| this.dispatch_ai_chat_event(AiChatWorkerEvent::AgentProtocolFallback { job_id, session_id: session_id.clone() }, cx)).is_err() { return; }
                        continue;
                    }
                    delta = rx.recv() => match delta { Some(delta) => delta, None => break },
                };
                let result = this.update(cx, |this, cx| {
                    if run_id.as_ref().is_some_and(|run| !this.ai.native_run_is_live(run, &session_id)) { return; }
                    if !delta.text_delta.is_empty() || delta.reasoning_delta.is_some() {
                        this.dispatch_ai_chat_event(AiChatWorkerEvent::Delta { job_id, session_id: session_id.clone(), text_delta: delta.text_delta, reasoning_delta: delta.reasoning_delta }, cx);
                    }
                    for tool in delta.tool_call_deltas {
                        this.dispatch_ai_chat_event(AiChatWorkerEvent::AgentToolCallDelta { job_id, session_id: session_id.clone(), tool_name: tool.name_delta, arguments_delta_len: tool.arguments_delta.len() }, cx);
                    }
                });
                if result.is_err() { return; }
            }
            if let Ok(result) = result_rx.await {
                let _ = this.update(cx, |this, cx| {
                    if run_id.as_ref().is_none_or(|run| this.ai.native_run_is_live(run, &session_id)) {
                        this.dispatch_ai_chat_event(AiChatWorkerEvent::Finished(Box::new(AiChatJobResult { job_id, session_id, result })), cx);
                    }
                });
            }
        }).detach();
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
    tx: mpsc::Sender<nyaterm_core::ai::AiChatStreamDelta>,
    fallback_tx: oneshot::Sender<()>,
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
    let completion = match stream_chat(&session, &request, &history, tx.clone(), cancel).await {
        Err(error)
            if error.kind == AiProviderErrorKind::UnsupportedCapability
                && error.tool_protocol_unsupported
                && !error.generated_content
                && request.mode == nyaterm_core::ai::AiMode::Agent
                && !request.options.agent_json_protocol
                && request
                    .options
                    .agent_context
                    .as_ref()
                    .is_none_or(|context| context.calls.is_empty()) =>
        {
            request.options.agent_json_protocol = true;
            let _ = fallback_tx.send(());
            stream_chat(&session, &request, &history, tx, cancel).await
        }
        result => result,
    }
    .map_err(|error| error.to_string())?;
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
