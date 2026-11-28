use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol::v1::ConnectionTo;
use agent_client_protocol::v1::schema::{
    AuthStatusRequest, AuthenticateRequest, CancelNotification, InitializeRequest,
    LoadSessionRequest, LogoutRequest, NewSessionRequest, PermissionOptionKind, PromptRequest,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    ResumeSessionRequest, SelectedPermissionOutcome, SessionNotification,
    SetSessionConfigOptionRequest, SetSessionModeRequest,
};
use agent_client_protocol::{Agent, JsonRpcMessage, Lines, UntypedMessage};
use futures::{AsyncWriteExt, SinkExt, StreamExt};
use nyaterm_core::ai::external_agents::{ExternalAgentProfile, ExternalSessionBinding};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::acp::{AgentError, discovery, policy};
use crate::external_process::{ExternalAgentChild, spawn_reader};

const MAX_FRAME_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Clone)]
pub enum AgentOperation {
    Inspect,
    Authenticate {
        method: String,
    },
    Logout,
    Prompt {
        text: String,
        model: Option<String>,
        binding: Option<Box<ExternalSessionBinding>>,
        legacy_session_id: Option<String>,
    },
}

#[derive(Clone)]
pub enum AgentEvent {
    Permission(super::permissions::AgentPermissionRequest),
    Capabilities {
        auth_methods: Vec<(String, String)>,
        logout: bool,
    },
    Authentication {
        authenticated: Option<bool>,
    },
    Session(ExternalSessionBinding),
    Models(Vec<(String, String)>),
    Delta {
        text: String,
        reasoning: Option<String>,
    },
    Tool {
        id: String,
        title: String,
        status: String,
    },
}

#[derive(Default)]
pub struct AgentRunResult {
    pub text: String,
    pub reasoning: Option<String>,
    pub binding: Option<ExternalSessionBinding>,
}

impl std::fmt::Debug for AgentRunResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentRunResult").finish_non_exhaustive()
    }
}

/// All callbacks run on the caller-owned worker, never an SDK reader thread.
#[allow(clippy::too_many_arguments)]
pub fn run(
    profile: &ExternalAgentProfile,
    operation: AgentOperation,
    helper: Option<&Path>,
    mcp_environment: HashMap<String, String>,
    cancel: &AtomicBool,
    timeout: Duration,
    on_event: &mut dyn FnMut(AgentEvent) -> Result<(), String>,
    on_cancel: &mut dyn FnMut(),
) -> Result<AgentRunResult, AgentError> {
    if !policy::platform_supported(profile) {
        return Err(AgentError::UnsupportedPlatform);
    }
    if cancel.load(Ordering::Acquire) {
        return Err(AgentError::Cancelled);
    }
    profile.validate().map_err(AgentError::Configuration)?;
    if matches!(operation, AgentOperation::Prompt { .. }) {
        if !profile.enabled {
            return Err(AgentError::Configuration("Agent is disabled"));
        }
        policy::admit_execution(profile)?;
    }
    let executable = discovery::resolve(&profile.executable)?;
    if !profile.strategy.manages_local_tools() {
        let expected = policy::expected_version(profile)?;
        if !policy::matches_version(&discovery::version(&executable, cancel)?, expected) {
            return Err(AgentError::UnsupportedVersion);
        }
    }
    let engine = if profile.strategy.0 == "claude" {
        Some(discovery::claude_engine(&executable, cancel)?)
    } else if profile.strategy.0 == "codex" {
        Some(discovery::codex_engine(&executable, cancel)?)
    } else {
        None
    };
    run_resolved(
        profile,
        &executable,
        engine.as_deref(),
        operation,
        helper,
        mcp_environment,
        cancel,
        timeout,
        on_event,
        on_cancel,
    )
}

#[allow(clippy::too_many_arguments)]
fn run_resolved(
    profile: &ExternalAgentProfile,
    executable: &Path,
    engine: Option<&Path>,
    operation: AgentOperation,
    helper: Option<&Path>,
    mcp_environment: HashMap<String, String>,
    cancel: &AtomicBool,
    timeout: Duration,
    on_event: &mut dyn FnMut(AgentEvent) -> Result<(), String>,
    on_cancel: &mut dyn FnMut(),
) -> Result<AgentRunResult, AgentError> {
    let cwd = match &profile.working_directory {
        Some(directory) => std::fs::canonicalize(directory)
            .map_err(|_| AgentError::Configuration("Working directory does not exist"))?,
        None => {
            let directory = std::env::temp_dir()
                .join("nyaterm-acp-workspaces")
                .join(&profile.id);
            std::fs::create_dir_all(&directory)
                .map_err(|_| AgentError::Configuration("Cannot create Agent workspace"))?;
            std::fs::canonicalize(directory)
                .map_err(|_| AgentError::Configuration("Invalid Agent workspace"))?
        }
    };
    let codex_launch = if profile.strategy.0 == "codex" {
        Some(super::codex_policy::CodexLaunch::prepare(
            profile,
            engine.ok_or(AgentError::UnsupportedVersion)?,
            &cwd,
            cancel,
        )?)
    } else {
        None
    };
    let mut command = Command::new(executable);
    command
        .args(&profile.args)
        .current_dir(&cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    policy::configure(&mut command, profile)?;
    if let Some(launch) = &codex_launch {
        launch.configure(&mut command);
    } else if let Some(engine) = engine {
        command.env("CLAUDE_CODE_EXECUTABLE", engine);
    }
    let mut child = ExternalAgentChild::spawn(&mut command).map_err(|_| AgentError::Transport)?;
    let stdin = child.take_stdin().ok_or(AgentError::Transport)?;
    let stdout = child.take_stdout().ok_or(AgentError::Transport)?;
    let stderr = child.take_stderr().ok_or(AgentError::Transport)?;
    let (mut line_tx, line_rx) = futures::channel::mpsc::channel(16);
    child.own_reader(
        spawn_reader("nyaterm-acp-stdout", move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut bytes = Vec::new();
                let result = reader
                    .by_ref()
                    .take(MAX_FRAME_BYTES + 1)
                    .read_until(b'\n', &mut bytes);
                let line = match result {
                    Ok(0) => break,
                    Ok(_) if bytes.len() as u64 <= MAX_FRAME_BYTES => String::from_utf8(bytes)
                        .map_err(|_| {
                            std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                "Invalid ACP encoding",
                            )
                        }),
                    _ => Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "Invalid ACP frame",
                    )),
                };
                let failed = line.is_err();
                if futures::executor::block_on(line_tx.send(line)).is_err() || failed {
                    break;
                }
            }
        })
        .map_err(|_| AgentError::Transport)?,
    );
    child.own_reader(
        spawn_reader("nyaterm-acp-stderr", move || {
            // Adapter stderr may contain prompts, credentials, or complete protocol frames.
            let _ = std::io::copy(&mut BufReader::new(stderr), &mut std::io::sink());
        })
        .map_err(|_| AgentError::Transport)?,
    );
    let outgoing = Box::pin(futures::sink::unfold(
        blocking::Unblock::new(stdin),
        async |mut writer, line: String| {
            writer.write_all(line.as_bytes()).await?;
            writer.write_all(b"\n").await?;
            writer.flush().await?;
            Ok::<_, std::io::Error>(writer)
        },
    ));
    let (event_tx, mut event_rx) = futures::channel::mpsc::channel(64);
    let active_session = Arc::new(Mutex::new(None::<String>));
    let peer = Arc::new(Mutex::new(None::<ConnectionTo<Agent>>));
    let outcome = Arc::new(Mutex::new(None::<Result<(), AgentError>>));
    let shared_session = active_session.clone();
    let notify_session = active_session.clone();
    let permission_session = active_session.clone();
    let shared_peer = peer.clone();
    let shared_outcome = outcome.clone();
    let profile = profile.clone();
    let cwd = cwd.to_string_lossy().into_owned();
    let server = helper.map(|helper| {
        let mut server = policy::mcp_server(helper, &mcp_environment);
        if profile.strategy.0 == "codex" {
            server["name"] = json!(super::codex_policy::SERVER_NAME);
        }
        server
    });
    let codex_permissions = Arc::new(Mutex::new(super::policy::CodexPermissions::default()));
    let notify_permissions = codex_permissions.clone();
    let is_codex = profile.strategy.0 == "codex";
    let generic = profile.strategy.manages_local_tools();
    let permission_profile = profile.id.clone();
    let permission_events = event_tx.clone();
    let permission_slots = Arc::new(tokio::sync::Semaphore::new(32));
    let mut notify_events = event_tx.clone();
    let stopped = Arc::new(AtomicBool::new(false));
    let permission_stopped = stopped.clone();
    let connection = agent_client_protocol::Client
        .v1()
        .on_receive_notification(
            async move |notification: SessionNotification, _cx| {
                let belongs = notify_session.lock().unwrap().as_deref()
                    == Some(notification.session_id.0.as_ref());
                if belongs {
                    let update = serde_json::to_value(notification.update)
                        .map_err(|_| agent_client_protocol::Error::internal_error())?;
                    if is_codex {
                        notify_permissions.lock().unwrap().observe(&update);
                    }
                    if let Some(event) = update_event(&update) {
                        notify_events
                            .send(event)
                            .await
                            .map_err(|_| agent_client_protocol::Error::internal_error())?;
                    }
                }
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            async move |request: RequestPermissionRequest, responder, cx| {
                let belongs = permission_session.lock().unwrap().as_deref()
                    == Some(request.session_id.0.as_ref());
                let tool = serde_json::to_value(&request.tool_call).unwrap_or(Value::Null);
                if generic
                    && belongs
                    && !permission_stopped.load(Ordering::Acquire)
                    && let Ok(slot) = permission_slots.clone().try_acquire_owned()
                {
                    let (approval, reply) = super::permissions::AgentPermissionRequest::new(
                        permission_profile.clone(),
                        &tool,
                        request
                            .options
                            .iter()
                            .any(|option| option.kind == PermissionOptionKind::AllowOnce),
                        permission_stopped.clone(),
                    );
                    let mut events = permission_events.clone();
                    let stopped = permission_stopped.clone();
                    // A handler must return immediately: awaiting UI here would block dispatch.
                    return cx.spawn(async move {
                        let _slot = slot;
                        let allow = events.send(AgentEvent::Permission(approval)).await.is_ok()
                            && reply.await.unwrap_or(false)
                            && !stopped.load(Ordering::Acquire);
                        responder.respond(permission_response(&request, allow))
                    });
                }
                let allow = belongs
                    && !permission_stopped.load(Ordering::Acquire)
                    && if is_codex {
                        let request_value = serde_json::to_value(&request).unwrap_or(Value::Null);
                        codex_permissions.lock().unwrap().authorize(&request_value)
                    } else {
                        !generic && policy::is_nyaterm_permission(&tool)
                    };
                responder.respond(permission_response(&request, allow))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(
            Lines::new(outgoing, line_rx),
            move |connection: ConnectionTo<Agent>| async move {
                *shared_peer.lock().unwrap() = Some(connection.clone());
                let result = exchange(
                    &connection,
                    &profile,
                    operation,
                    &cwd,
                    server,
                    event_tx,
                    shared_session,
                )
                .await;
                *shared_outcome.lock().unwrap() = Some(result);
                Ok(())
            },
        );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| AgentError::Transport)?;
    let result = runtime.block_on(async {
        tokio::pin!(connection);
        let deadline = tokio::time::Instant::now() + timeout;
        let mut cancelled_at = None;
        let mut timed_out = false;
        let mut result = AgentRunResult::default();
        let mut tick = tokio::time::interval(Duration::from_millis(25));
        loop {
            tokio::select! {
                connected = &mut connection => {
                    while let Ok(event) = event_rx.try_recv() {
                        apply_event(event, &mut result, on_event)?;
                    }
                    if cancelled_at.is_some() { return Err(if timed_out {AgentError::Timeout} else {AgentError::Cancelled}); }
                    connected.map_err(|_| AgentError::Transport)?;
                    outcome.lock().unwrap().take().ok_or(AgentError::Transport)??;
                    return Ok(result);
                }
                Some(event) = event_rx.next() => apply_event(event, &mut result, on_event)?,
                _ = tick.tick() => {
                    let now = tokio::time::Instant::now();
                    if (now >= deadline || cancel.load(Ordering::Acquire)) && cancelled_at.is_none() {
                        timed_out = now >= deadline;
                        stopped.store(true, Ordering::Release);
                        on_cancel();
                        cancelled_at = Some(now);
                        if let (Some(peer), Some(session)) = (peer.lock().unwrap().as_ref(), active_session.lock().unwrap().as_ref()) {
                            let _ = peer.send_notification(CancelNotification::new(session.clone()));
                        }
                    }
                    if cancelled_at.is_some_and(|at| now.duration_since(at) >= Duration::from_secs(5)) {
                        return Err(if timed_out {AgentError::Timeout} else {AgentError::Cancelled});
                    }
                }
            }
        }
    });
    if result.is_err() && !stopped.swap(true, Ordering::AcqRel) {
        on_cancel();
    }
    stopped.store(true, Ordering::Release);
    drop(event_rx);
    drop(peer);
    // Readers are joined only after the complete child tree has been terminated.
    drop(child);
    result
}

fn apply_event(
    event: AgentEvent,
    result: &mut AgentRunResult,
    sink: &mut dyn FnMut(AgentEvent) -> Result<(), String>,
) -> Result<(), AgentError> {
    match &event {
        AgentEvent::Delta { text, reasoning } => {
            let total = result.text.len()
                + result.reasoning.as_ref().map_or(0, String::len)
                + text.len()
                + reasoning.as_ref().map_or(0, String::len);
            if total > 16 * 1024 * 1024 {
                return Err(AgentError::Protocol("response exceeds size limit"));
            }
            result.text.push_str(text);
            if let Some(reasoning) = reasoning {
                result.reasoning.get_or_insert_default().push_str(reasoning);
            }
        }
        AgentEvent::Session(binding) => result.binding = Some(binding.clone()),
        _ => {}
    }
    sink(event).map_err(|_| AgentError::Protocol("event delivery"))
}

fn update_event(update: &Value) -> Option<AgentEvent> {
    match update["sessionUpdate"].as_str()? {
        "agent_message_chunk" | "agent_thought_chunk" => {
            let text = update["content"]["text"].as_str()?.to_owned();
            Some(if update["sessionUpdate"] == "agent_thought_chunk" {
                AgentEvent::Delta {
                    text: String::new(),
                    reasoning: Some(text),
                }
            } else {
                AgentEvent::Delta {
                    text,
                    reasoning: None,
                }
            })
        }
        "tool_call" | "tool_call_update" => Some(AgentEvent::Tool {
            id: update["toolCallId"].as_str()?.into(),
            title: update["title"].as_str().unwrap_or_default().into(),
            status: update["status"].as_str().unwrap_or("pending").into(),
        }),
        _ => None,
    }
}

fn permission_response(
    request: &RequestPermissionRequest,
    allow: bool,
) -> RequestPermissionResponse {
    let desired = if allow {
        PermissionOptionKind::AllowOnce
    } else {
        PermissionOptionKind::RejectOnce
    };
    let outcome = request
        .options
        .iter()
        .find(|option| option.kind == desired)
        .map(|option| {
            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                option.option_id.clone(),
            ))
        })
        .unwrap_or(RequestPermissionOutcome::Cancelled);
    RequestPermissionResponse::new(outcome)
}

// SDK 5 removed the legacy `models` response field. Preserve it for older ACP agents.
fn preserve_response(request: impl JsonRpcMessage) -> Result<UntypedMessage, AgentError> {
    request
        .to_untyped_message()
        .map_err(|_| AgentError::Protocol("request encoding"))
}

fn decode<T: DeserializeOwned>(value: Value) -> Result<T, AgentError> {
    serde_json::from_value(value).map_err(|_| AgentError::Protocol("request encoding"))
}

#[allow(clippy::too_many_arguments)]
async fn exchange(
    peer: &ConnectionTo<Agent>,
    profile: &ExternalAgentProfile,
    operation: AgentOperation,
    cwd: &str,
    server: Option<Value>,
    mut events: futures::channel::mpsc::Sender<AgentEvent>,
    active_session: Arc<Mutex<Option<String>>>,
) -> Result<(), AgentError> {
    let init = peer.send_request(decode::<InitializeRequest>(json!({
        "protocolVersion":1, "clientInfo":{"name":"nyaterm","version":env!("CARGO_PKG_VERSION")},
        "clientCapabilities":{}
    }))?).block_task().await.map_err(|_| AgentError::Protocol("initialize"))?;
    let init = serde_json::to_value(init).map_err(|_| AgentError::Protocol("initialize"))?;
    if init["protocolVersion"] != 1 {
        return Err(AgentError::Protocol("protocol version"));
    }
    let methods = init["authMethods"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|method| {
            Some((
                method["id"].as_str()?.into(),
                method["name"].as_str()?.into(),
            ))
        })
        .collect();
    events
        .send(AgentEvent::Capabilities {
            auth_methods: methods,
            logout: init
                .pointer("/agentCapabilities/auth/logout")
                .is_some_and(Value::is_object),
        })
        .await
        .map_err(|_| AgentError::Transport)?;
    if init
        .pointer("/agentCapabilities/auth/status")
        .is_some_and(Value::is_object)
    {
        for attempt in 0..10 {
            let status = peer
                .send_request(decode::<AuthStatusRequest>(json!({}))?)
                .block_task()
                .await
                .map_err(|_| AgentError::Protocol("auth status"))?;
            let value =
                serde_json::to_value(status).map_err(|_| AgentError::Protocol("auth status"))?;
            let authenticated = match value["state"].as_str() {
                Some("authenticated" | "not_required") => Some(true),
                Some("unauthenticated") => Some(false),
                _ => None,
            };
            if authenticated.is_some() || attempt == 9 {
                events
                    .send(AgentEvent::Authentication { authenticated })
                    .await
                    .map_err(|_| AgentError::Transport)?;
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
    let (text, model, binding, legacy_session_id) = match operation {
        AgentOperation::Inspect => return Ok(()),
        AgentOperation::Authenticate { method } => {
            peer.send_request(decode::<AuthenticateRequest>(json!({"methodId":method}))?)
                .block_task()
                .await
                .map_err(|_| AgentError::Protocol("authenticate"))?;
            return Ok(());
        }
        AgentOperation::Logout => {
            if init.pointer("/agentCapabilities/auth/logout").is_none() {
                return Err(AgentError::Protocol("logout unsupported"));
            }
            peer.send_request(decode::<LogoutRequest>(json!({}))?)
                .block_task()
                .await
                .map_err(|_| AgentError::Protocol("logout"))?;
            return Ok(());
        }
        AgentOperation::Prompt {
            text,
            model,
            binding,
            legacy_session_id,
        } => (text, model, binding, legacy_session_id),
    };
    let server = server.ok_or(AgentError::Configuration("NyaTerm MCP is required"))?;
    // Private-protocol histories have no evidence of strict launch policy. Preserve them
    // as readable transcripts and require an explicit new conversation from their text.
    if legacy_session_id.is_some() {
        return Err(AgentError::Resume);
    }

    if binding
        .as_ref()
        .is_some_and(|binding| !binding.can_resume(profile, cwd))
    {
        return Err(AgentError::Resume);
    }
    let existing = binding
        .map(|binding| binding.external_id)
        .or(legacy_session_id);
    let mut setup =
        json!({"cwd":cwd, "mcpServers":[server], "_meta":policy::session_metadata(profile)});
    let (session_id, options) = if let Some(id) = existing {
        setup["sessionId"] = json!(id);
        // Publish no active session until restore succeeds, so replay cannot duplicate local history.
        let options = if init
            .pointer("/agentCapabilities/sessionCapabilities/resume")
            .is_some_and(Value::is_object)
        {
            serde_json::to_value(
                peer.send_request(preserve_response(decode::<ResumeSessionRequest>(setup)?)?)
                    .block_task()
                    .await
                    .map_err(|_| AgentError::Resume)?,
            )
        } else if init["agentCapabilities"]["loadSession"] == true {
            serde_json::to_value(
                peer.send_request(preserve_response(decode::<LoadSessionRequest>(setup)?)?)
                    .block_task()
                    .await
                    .map_err(|_| AgentError::Resume)?,
            )
        } else {
            return Err(AgentError::Resume);
        }
        .map_err(|_| AgentError::Protocol("restore"))?;
        (id, options)
    } else {
        let created = peer
            .send_request(preserve_response(decode::<NewSessionRequest>(setup)?)?)
            .block_task()
            .await
            .map_err(|_| AgentError::Protocol("new session"))?;
        let value =
            serde_json::to_value(created).map_err(|_| AgentError::Protocol("new session"))?;
        (
            value["sessionId"]
                .as_str()
                .ok_or(AgentError::Protocol("session id"))?
                .to_owned(),
            value,
        )
    };
    if matches!(profile.strategy.0.as_str(), "claude" | "codex") {
        let strict_mode = if profile.strategy.0 == "codex" {
            "read-only"
        } else {
            "default"
        };
        // The adapter chooses its own initial mode; SDK options.permissionMode is overwritten.
        // Set the negotiated mode explicitly before the first prompt, including resumed sessions.
        if !options
            .pointer("/modes/availableModes")
            .and_then(Value::as_array)
            .is_some_and(|modes| modes.iter().any(|mode| mode["id"] == strict_mode))
        {
            return Err(AgentError::Protocol("strict approval mode unavailable"));
        }
        peer.send_request(decode::<SetSessionModeRequest>(
            json!({"sessionId":session_id,"modeId":strict_mode}),
        )?)
        .block_task()
        .await
        .map_err(|_| AgentError::Protocol("strict approval mode"))?;
    }
    *active_session.lock().unwrap() = Some(session_id.clone());
    let binding = ExternalSessionBinding {
        profile_id: profile.id.clone(),
        protocol: "acp_v1".into(),
        external_id: session_id.clone(),
        working_directory: cwd.into(),
        config_directory: profile.config_directory.clone(),
        source_session_id: None,
        revision: profile.revision,
        extra: Default::default(),
    };
    events
        .send(AgentEvent::Session(binding))
        .await
        .map_err(|_| AgentError::Transport)?;
    let model_config = options["configOptions"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|option| option["category"] == "model" || option["id"] == "model");
    let models = model_config.map(model_choices).unwrap_or_else(|| {
        options
            .pointer("/models/availableModels")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|model| {
                Some((
                    model["modelId"].as_str()?.into(),
                    model["name"].as_str()?.into(),
                ))
            })
            .collect()
    });
    events
        .send(AgentEvent::Models(models.clone()))
        .await
        .map_err(|_| AgentError::Transport)?;
    if let Some(model) = model
        .or_else(|| profile.default_model.clone())
        .filter(|value| !value.is_empty())
    {
        if !models.iter().any(|(id, _)| id == &model) {
            return Err(AgentError::Protocol("model is not available"));
        }
        if let Some(config_id) = model_config.and_then(|c| c["id"].as_str()) {
            peer.send_request(decode::<SetSessionConfigOptionRequest>(
                json!({"sessionId":session_id,"configId":config_id,"value":model}),
            )?)
            .block_task()
            .await
            .map_err(|_| AgentError::Protocol("model selection"))?;
        } else {
            peer.send_request(
                UntypedMessage::new(
                    "session/set_model",
                    json!({"sessionId":session_id,"modelId":model}),
                )
                .map_err(|_| AgentError::Protocol("model selection"))?,
            )
            .block_task()
            .await
            .map_err(|_| AgentError::Protocol("model selection"))?;
        }
    }
    let response = peer
        .send_request(decode::<PromptRequest>(
            json!({"sessionId":session_id,"prompt":[{"type":"text","text":text}]}),
        )?)
        .block_task()
        .await
        .map_err(|_| AgentError::Protocol("prompt"))?;
    let response = serde_json::to_value(response).map_err(|_| AgentError::Protocol("prompt"))?;
    match response["stopReason"].as_str() {
        Some("end_turn") => Ok(()),
        Some("cancelled") => Err(AgentError::Cancelled),
        _ => Err(AgentError::Incomplete),
    }
}

fn model_choices(config: &Value) -> Vec<(String, String)> {
    let mut choices = Vec::new();
    for option in config["options"].as_array().into_iter().flatten() {
        if let (Some(value), Some(name)) = (option["value"].as_str(), option["name"].as_str()) {
            choices.push((value.into(), name.into()));
        } else {
            choices.extend(model_choices(option));
        }
    }
    choices
}

#[cfg(all(test, windows))]
mod tests;
