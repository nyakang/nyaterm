//! Grouped AI feature state.
//!
//! The AI panel spans several independent concerns: provider settings, the
//! chat composer and transcript, session history, model discovery, and the
//! agent loop. They were seventy `ai_*` fields on `NyaTermApp`, which made it
//! impossible to see which ones move together.

pub(in crate::features) mod harness;
mod presentation;
mod providers;
mod settings;
pub(in crate::features) use providers::{ConnectionStatus, ProviderSettingsView};

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use std::time::Instant;

use gpui::{FocusHandle, Pixels};
use nyaterm_core::{
    AgentCommandExecutionMode, AiAgentKind, AiCommandCard, AiMessage, AiMessageRole, AiMode,
    AiSession, AiSettings, truncate_preview, uuid,
};

use super::agent_management::{
    AgentCommand, AgentJobEvent, AgentManagementState, AgentManagementView,
};
use super::presentation::{AiAgentStepKind, AiResponsePhase};
use crate::features::{
    runtime_jobs::AiAgentLoopState, runtime_jobs::AiAgentStepStatus, runtime_jobs::AiAgentStepView,
    runtime_jobs::AiChatJobOutput, runtime_jobs::AiChatWorkerEvent,
    runtime_jobs::AiDiscoveryJobResult,
};
use crate::models::{
    AiActionEditorField, AiActionListKind, AiDetectedErrorState, AiInputField, AiPreparedRequest,
};

pub(in crate::features) struct AiFeatureState {
    settings: AiSettingsState,
    chat: AiChatState,
    active_scope_key: String,
    visible_scope_epoch: Arc<AtomicU64>,
    inactive_scopes: HashMap<String, AiConversationState>,
    archived_sessions: HashMap<String, (String, AiConversationState)>,
    history: AiHistoryState,
    discovery: AiDiscoveryState,
    agent: AiAgentState,
    agent_management: AgentManagementState,
    panel: AiPanelState,
}

/// Focus handles the AI feature needs at construction time.
pub(in crate::features) struct AiFeatureFocus {
    pub chat: FocusHandle,
    pub manual_model: FocusHandle,
}

pub(in crate::features) struct AiFeatureInit {
    pub settings: AiSettings,
    pub model_draft: String,
    pub base_url_draft: String,
    pub chat_session_id: String,
    pub session_count: usize,
    pub message_count: usize,
    pub audit_count: usize,
}

/// Provider settings, model catalog editing and credential drafts.
struct AiSettingsState {
    providers: ProviderSettingsView,
    config: AiSettings,
    model_draft: String,
    base_url_draft: String,
    secret_draft: nyaterm_core::SecretString,
    manual_model_drafts: HashMap<String, String>,
    manual_model_focus: FocusHandle,
    manual_model_edit_group: Option<String>,
    /// Per-credential API-key drafts; empty means keep the stored secret.
    credential_secret_drafts: HashMap<String, String>,
    /// Secret drafts scoped to the credential and header identities.
    credential_header_secret_drafts: HashMap<(String, String), nyaterm_core::SecretString>,
    action_edit: Option<(AiActionListKind, String, AiActionEditorField)>,
    persistence_generation: u64,
    persistence_in_flight: Option<u64>,
    persistence_pending: Option<AiSettings>,
    persistence_dirty: bool,
    pending_full_access: Option<AiFullAccessSetting>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::features) enum AiFullAccessSetting {
    ExternalAgent,
    Codex,
    ClaudeCode,
    McpHost,
}

pub(in crate::features) struct AiSettingsPersistenceCompletion {
    pub(in crate::features) apply_result: bool,
    pub(in crate::features) report_result: bool,
    pub(in crate::features) next: Option<(u64, AiSettings)>,
}

/// Composer, in-flight request and the visible transcript.
struct AiChatState {
    native_task: Option<crate::ai_runtime::AiTask>,
    response_metadata: HashMap<String, nyaterm_core::ai::completion::AiResponseMetadata>,
    provider_session: Option<Arc<crate::http::ai::AiProviderSession>>,
    tx: UnboundedSender<AiChatWorkerEvent>,
    /// Taken once by `NyaTermApp::start_ai_chat_event_drain`, which owns
    /// delivery from then on. `None` afterwards, so a second start is a no-op.
    rx: Option<UnboundedReceiver<AiChatWorkerEvent>>,
    pending: bool,
    job_id: u64,
    cancel: Option<Arc<AtomicBool>>,
    session_id: String,
    run_mode: AiMode,
    agent_kind: AiAgentKind,
    prompt_draft: String,
    target_session_ids: Vec<String>,
    mention_open: bool,
    mention_query: String,
    mention_index: usize,
    prepared_request: Option<AiPreparedRequest>,
    response_preview: String,
    messages: Vec<Arc<AiMessage>>,
    streaming_assistant_id: Option<String>,
    response_phase: AiResponsePhase,
    thought_expanded: HashSet<String>,
    command_details_expanded: HashSet<String>,
    command_scripts_expanded: HashSet<String>,
    agent_history_expanded: bool,
    execution_groups_expanded: HashSet<String>,
    quoted_text: Option<String>,
    command_cards: Vec<AiCommandCard>,
    focus: FocusHandle,
    focus_pending: bool,
}

struct AiConversationState {
    chat: AiChatState,
    agent: AiAgentState,
    status: String,
}

impl AiChatState {
    fn fresh(
        tx: UnboundedSender<AiChatWorkerEvent>,
        focus: FocusHandle,
        run_mode: AiMode,
        agent_kind: AiAgentKind,
    ) -> Self {
        Self {
            tx,
            rx: None,
            native_task: None,
            response_metadata: HashMap::new(),
            provider_session: None,
            pending: false,
            job_id: 0,
            cancel: None,
            session_id: format!("ai-session-{}", uuid()),
            run_mode,
            agent_kind,
            prompt_draft: String::new(),
            target_session_ids: Vec::new(),
            mention_open: false,
            mention_query: String::new(),
            mention_index: 0,
            prepared_request: None,
            response_preview: "Ask mode ready".to_string(),
            messages: Vec::new(),
            streaming_assistant_id: None,
            response_phase: AiResponsePhase::Ended,
            thought_expanded: HashSet::new(),
            command_details_expanded: HashSet::new(),
            command_scripts_expanded: HashSet::new(),
            agent_history_expanded: false,
            execution_groups_expanded: HashSet::new(),
            quoted_text: None,
            command_cards: Vec::new(),
            focus,
            focus_pending: false,
        }
    }
}

/// Stored sessions, the history browser and the counters shown beside it.
struct AiHistoryState {
    open: bool,
    restore_focus: Option<FocusHandle>,
    error: Option<String>,
    query: String,
    job_id: u64,
    pending: bool,
    sessions: Vec<AiSession>,
    session_count: usize,
    message_count: usize,
    audit_count: usize,
    usage_count_job_id: u64,
    audit_write_lock: Arc<Mutex<()>>,
}

/// Model discovery job and the model picker it feeds.
struct AiDiscoveryState {
    #[cfg(test)]
    tx: UnboundedSender<AiDiscoveryJobResult>,
    /// Taken once by `NyaTermApp::start_ai_discovery_event_drain`, which owns
    /// delivery from then on. `None` afterwards, so a second start is a no-op.
    rx: Option<UnboundedReceiver<AiDiscoveryJobResult>>,
    pending: bool,
    menu_open: bool,
    query: String,
    index: usize,
}

/// Agent loop: the running task, its steps and their disclosure state.
struct AiAgentState {
    native: Option<harness::NativeRunState>,
    task_prompt: Option<String>,
    /// Legacy runs only; native runs own their language in NativeRunState::request.
    task_language: Option<String>,
    conversation: Vec<AiMessage>,
    json_protocol: bool,
    step_index: u16,
    loop_state: Option<AiAgentLoopState>,
    steps: Vec<AiAgentStepView>,
    thought_expanded: HashSet<u16>,
    output_expanded: HashSet<u16>,
}

impl AiAgentState {
    fn fresh() -> Self {
        Self {
            native: None,
            task_prompt: None,
            task_language: None,
            conversation: Vec::new(),
            json_protocol: false,
            step_index: 0,
            loop_state: None,
            steps: Vec::new(),
            thought_expanded: HashSet::new(),
            output_expanded: HashSet::new(),
        }
    }
}

pub(in crate::features) struct AiChatLaunch {
    pub(in crate::features) job_id: u64,
    pub(in crate::features) cancel: Arc<AtomicBool>,
    pub(in crate::features) tx: UnboundedSender<AiChatWorkerEvent>,
    pub(in crate::features) session_id: String,
}

pub(in crate::features) struct AiChatFinishEffect {
    pub(in crate::features) session_id: String,
    pub(in crate::features) succeeded: bool,
    pub(in crate::features) clear_prompt_input: bool,
    pub(in crate::features) refresh_usage_counts: bool,
    pub(in crate::features) auto_execute_first: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::features) enum AiSettingsMutation {
    Ignored,
    Notify,
    Persist,
}

/// Panel chrome: status line, focus routing and the detected-error banner.
struct AiPanelState {
    execution_menu_open: bool,
    /// Agent runtime restored when the composer switches from Ask back to
    /// Agent. Ask persists `Nyaterm` as the default kind, so this choice is
    /// remembered for the process lifetime only.
    last_agent_kind: AiAgentKind,
    status: String,
    focused_field: AiInputField,
    detected_error: Option<AiDetectedErrorState>,
    error_notice_at: HashMap<String, Instant>,
    panel_refresh_requested: bool,
    stream_refresh_generation: u64,
    stream_refresh_pending: bool,
    /// Manual floor for the composer prompt box. `0` leaves the content-driven
    /// auto-grow untouched, which also doubles as the resize's lower bound.
    composer_min_height: f32,
    composer_resize: Option<AiComposerResize>,
}

/// Pointer-driven composer height drag. `start_height` is the box's measured
/// height at press so the floor tracks the visible top edge, not a stale floor.
#[derive(Clone, Copy)]
struct AiComposerResize {
    start_y: Pixels,
    start_height: Pixels,
    start_floor: f32,
}

fn non_empty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then(|| value.trim().to_string())
}

impl AiFeatureState {
    pub(in crate::features) fn cancel_removed_provider_runs(&mut self, config: &AiSettings) {
        let visible = self.active_scope_key.clone();
        let mut scopes = self.inactive_scopes.keys().cloned().collect::<Vec<_>>();
        scopes.push(visible.clone());
        for scope in scopes {
            self.switch_scope(&scope);
            if self.chat.provider_session.as_ref().is_some_and(|session| {
                !config.provider_credentials.iter().any(|credential| {
                    credential.id == session.credential_id() && credential.enabled
                })
            }) {
                self.cancel_chat_and_agent();
                self.chat.provider_session = None;
            }
        }
        self.switch_scope(&visible);
    }
    pub(in crate::features) fn run_settings(&self) -> &AiSettings {
        self.chat
            .provider_session
            .as_ref()
            .map(|session| &session.settings)
            .unwrap_or(&self.settings.config)
    }

    pub(in crate::features) fn provider_session(
        &mut self,
        request: &nyaterm_core::ai::AiChatRequest,
    ) -> Result<Arc<crate::http::ai::AiProviderSession>, String> {
        if let Some(session) = &self.chat.provider_session {
            return Ok(session.clone());
        }
        let session =
            crate::http::ai::AiProviderSession::new(self.settings.config.clone(), request)
                .map_err(|error| error.to_string())?;
        self.chat.provider_session = Some(session.clone());
        Ok(session)
    }
    pub(in crate::features) fn native_network_cancel(&self) -> tokio_util::sync::CancellationToken {
        self.agent
            .native
            .as_ref()
            .map(|native| native.cancellation.child_token())
            .unwrap_or_default()
    }
    pub(in crate::features) fn own_native_task(&mut self, task: crate::ai_runtime::AiTask) {
        self.chat.native_task = Some(task);
    }

    pub(in crate::features) fn new(init: AiFeatureInit, focus: AiFeatureFocus) -> Self {
        let AiFeatureInit {
            settings,
            model_draft,
            base_url_draft,
            chat_session_id,
            session_count,
            message_count,
            audit_count,
        } = init;
        let (chat_tx, chat_rx) = unbounded();
        let (_discovery_tx, discovery_rx) = unbounded();
        let default_mode = settings.default_mode.clone();
        let default_agent_kind = settings.default_agent_kind.clone();
        let last_agent_kind_seed = default_agent_kind.clone();
        Self {
            settings: AiSettingsState {
                providers: ProviderSettingsView::default(),
                config: settings,
                model_draft,
                base_url_draft,
                secret_draft: nyaterm_core::SecretString::default(),
                manual_model_drafts: HashMap::new(),
                manual_model_focus: focus.manual_model,
                manual_model_edit_group: None,
                credential_secret_drafts: HashMap::new(),
                credential_header_secret_drafts: HashMap::new(),
                action_edit: None,
                persistence_generation: 0,
                persistence_in_flight: None,
                persistence_pending: None,
                persistence_dirty: false,
                pending_full_access: None,
            },
            chat: AiChatState {
                native_task: None,
                response_metadata: HashMap::new(),
                provider_session: None,
                tx: chat_tx,
                rx: Some(chat_rx),
                pending: false,
                job_id: 0,
                cancel: None,
                session_id: chat_session_id,
                run_mode: default_mode,
                agent_kind: default_agent_kind,
                prompt_draft: String::new(),
                target_session_ids: Vec::new(),
                mention_open: false,
                mention_query: String::new(),
                mention_index: 0,
                prepared_request: None,
                response_preview: "Ask mode ready".to_string(),
                messages: Vec::new(),
                streaming_assistant_id: None,
                response_phase: AiResponsePhase::Ended,
                thought_expanded: HashSet::new(),
                command_details_expanded: HashSet::new(),
                command_scripts_expanded: HashSet::new(),
                agent_history_expanded: false,
                execution_groups_expanded: HashSet::new(),
                quoted_text: None,
                command_cards: Vec::new(),
                focus: focus.chat,
                focus_pending: false,
            },
            active_scope_key: "unbound:".to_string(),
            visible_scope_epoch: Arc::new(AtomicU64::new(0)),
            inactive_scopes: HashMap::new(),
            archived_sessions: HashMap::new(),
            history: AiHistoryState {
                open: false,
                restore_focus: None,
                error: None,
                query: String::new(),
                job_id: 0,
                pending: false,
                sessions: Vec::new(),
                session_count,
                message_count,
                audit_count,
                usage_count_job_id: 0,
                audit_write_lock: Arc::new(Mutex::new(())),
            },
            discovery: AiDiscoveryState {
                #[cfg(test)]
                tx: _discovery_tx,
                rx: Some(discovery_rx),
                pending: false,
                menu_open: false,
                query: String::new(),
                index: 0,
            },
            agent: AiAgentState::fresh(),
            agent_management: AgentManagementState::new(),
            panel: AiPanelState {
                execution_menu_open: false,
                last_agent_kind: last_agent_kind_seed,
                status: "AI settings ready".to_string(),
                focused_field: AiInputField::Model,
                detected_error: None,
                error_notice_at: HashMap::new(),
                panel_refresh_requested: false,
                stream_refresh_generation: 0,
                stream_refresh_pending: false,
                composer_min_height: 0.,
                composer_resize: None,
            },
        }
    }

    pub(in crate::features) fn active_scope_key(&self) -> &str {
        &self.active_scope_key
    }

    pub(in crate::features) fn visible_scope_guard(&self) -> (Arc<AtomicU64>, u64) {
        (
            Arc::clone(&self.visible_scope_epoch),
            self.visible_scope_epoch.load(Ordering::Acquire),
        )
    }

    pub(in crate::features) fn switch_visible_scope(&mut self, scope_key: &str) -> bool {
        if !self.switch_scope(scope_key) {
            return false;
        }
        self.visible_scope_epoch.fetch_add(1, Ordering::AcqRel);
        true
    }

    pub(in crate::features) fn switch_scope(&mut self, scope_key: &str) -> bool {
        if self.active_scope_key == scope_key {
            return false;
        }
        let next = self
            .inactive_scopes
            .remove(scope_key)
            .unwrap_or_else(|| AiConversationState {
                chat: AiChatState::fresh(
                    self.chat.tx.clone(),
                    self.chat.focus.clone(),
                    self.settings.config.default_mode.clone(),
                    self.settings.config.default_agent_kind.clone(),
                ),
                agent: AiAgentState::fresh(),
                status: "AI assistant ready".to_string(),
            });
        let mut previous = AiConversationState {
            chat: std::mem::replace(&mut self.chat, next.chat),
            agent: std::mem::replace(&mut self.agent, next.agent),
            status: std::mem::replace(&mut self.panel.status, next.status),
        };
        if self.chat.rx.is_none() {
            self.chat.rx = previous.chat.rx.take();
        }
        self.inactive_scopes.insert(
            std::mem::replace(&mut self.active_scope_key, scope_key.to_string()),
            previous,
        );
        true
    }

    pub(in crate::features) fn scope_for_ai_session(&self, session_id: &str) -> Option<String> {
        if self.chat.session_id == session_id {
            return Some(self.active_scope_key.clone());
        }
        self.inactive_scopes
            .iter()
            .find(|(_, state)| state.chat.session_id == session_id)
            .map(|(scope, _)| scope.clone())
            .or_else(|| {
                self.archived_sessions
                    .get(session_id)
                    .map(|(scope, _)| scope.clone())
            })
    }

    pub(in crate::features) fn ai_session_is_running(&self, session_id: &str) -> bool {
        if self.chat.session_id == session_id {
            return self.chat_or_agent_is_running();
        }
        self.inactive_scopes.values().any(|scope| {
            scope.chat.session_id == session_id
                && (scope.chat.pending
                    || scope.agent.loop_state.is_some()
                    || scope.agent.task_prompt.is_some())
        })
    }

    pub(in crate::features) fn release_ai_session(&mut self, session_id: &str) {
        if self.chat.session_id == session_id {
            return;
        }
        if let Some((_, scope)) = self.inactive_scopes.iter_mut().find(|(_, scope)| {
            scope.chat.session_id == session_id
                && !scope.chat.pending
                && scope.agent.loop_state.is_none()
        }) {
            scope.chat = AiChatState::fresh(
                scope.chat.tx.clone(),
                scope.chat.focus.clone(),
                self.settings.config.default_mode.clone(),
                self.settings.config.default_agent_kind.clone(),
            );
            scope.agent = AiAgentState::fresh();
            scope.status = "AI assistant ready".to_string();
        }
    }

    fn archive_current_session(&mut self) {
        if self.chat.messages.is_empty() || self.chat_or_agent_is_running() {
            return;
        }
        let mut fresh = AiChatState::fresh(
            self.chat.tx.clone(),
            self.chat.focus.clone(),
            self.chat.run_mode.clone(),
            self.chat.agent_kind.clone(),
        );
        fresh.rx = self.chat.rx.take();
        let chat = std::mem::replace(&mut self.chat, fresh);
        let agent = std::mem::replace(&mut self.agent, AiAgentState::fresh());
        let status = std::mem::replace(&mut self.panel.status, "AI assistant ready".to_string());
        self.archived_sessions.insert(
            chat.session_id.clone(),
            (
                self.active_scope_key.clone(),
                AiConversationState {
                    chat,
                    agent,
                    status,
                },
            ),
        );
    }

    fn restore_archived_session(&mut self, session_id: &str) -> bool {
        if !self
            .archived_sessions
            .get(session_id)
            .is_some_and(|(scope, _)| scope == &self.active_scope_key)
        {
            return false;
        }
        let Some((_, mut archived)) = self.archived_sessions.remove(session_id) else {
            return false;
        };
        self.archive_current_session();
        archived.chat.rx = self.chat.rx.take();
        self.chat = archived.chat;
        self.agent = archived.agent;
        self.panel.status = archived.status;
        true
    }

    pub(in crate::features) fn any_chat_or_agent_running(&self) -> bool {
        self.chat_or_agent_is_running()
            || self.inactive_scopes.values().any(|state| {
                state.chat.pending
                    || state.agent.loop_state.is_some()
                    || state.agent.task_prompt.is_some()
            })
    }

    pub(in crate::features) fn agent_target_busy(&self, terminal_session_id: &str) -> bool {
        self.agent
            .loop_state
            .as_ref()
            .is_some_and(|state| state.terminal_session_id == terminal_session_id)
            || self.inactive_scopes.values().any(|scope| {
                scope
                    .agent
                    .loop_state
                    .as_ref()
                    .is_some_and(|state| state.terminal_session_id == terminal_session_id)
            })
    }

    pub(in crate::features) fn chat_or_agent_is_running(&self) -> bool {
        self.chat.pending || self.agent.loop_state.is_some() || self.agent.task_prompt.is_some()
    }

    pub(in crate::features) fn chat_or_agent_is_running_in_scope(&self, scope: &str) -> bool {
        if scope == self.active_scope_key {
            return self.chat_or_agent_is_running();
        }
        self.inactive_scopes.get(scope).is_some_and(|state| {
            state.chat.pending
                || state.agent.loop_state.is_some()
                || state.agent.task_prompt.is_some()
        })
    }

    pub(in crate::features) fn agent_task_is_active(&self) -> bool {
        self.agent.task_prompt.is_some()
    }

    pub(in crate::features) fn current_agent_command_card(&self, card_id: &str) -> bool {
        self.agent_task_is_active()
            && self.agent.steps.iter().any(|step| {
                step.command_card_id.as_deref() == Some(card_id)
                    && (step.status == AiAgentStepStatus::NeedsApproval
                        || (step.status == AiAgentStepStatus::Running
                            && step.kind == AiAgentStepKind::ToolProgress))
            })
            && self
                .chat
                .command_cards
                .iter()
                .any(|card| card.id == card_id)
    }

    pub(in crate::features) fn chat_is_pending(&self) -> bool {
        self.chat.pending
    }

    pub(in crate::features) fn chat_focus(&self) -> &FocusHandle {
        &self.chat.focus
    }

    pub(in crate::features) fn chat_focus_is_pending(&self) -> bool {
        self.chat.focus_pending
    }

    pub(in crate::features) fn take_chat_focus_request(&mut self) -> bool {
        std::mem::take(&mut self.chat.focus_pending)
    }

    pub(in crate::features) fn chat_session_id(&self) -> &str {
        &self.chat.session_id
    }

    pub(in crate::features) fn chat_run_mode(&self) -> AiMode {
        if self.chat.run_mode == AiMode::Agent
            && match self.chat.agent_kind {
                AiAgentKind::Codex => !self.settings.config.codex.enabled,
                AiAgentKind::ClaudeCode => !self.settings.config.claude_code.enabled,
                AiAgentKind::Nyaterm => false,
            }
        {
            return AiMode::Ask;
        }
        self.chat.run_mode.clone()
    }

    pub(in crate::features) fn chat_agent_kind(&self) -> AiAgentKind {
        if self.chat_run_mode() == AiMode::Ask {
            AiAgentKind::Nyaterm
        } else {
            self.chat.agent_kind.clone()
        }
    }

    /// The agent runtime to restore when switching from Ask to Agent, falling
    /// back to NyaTerm when the remembered external runtime was disabled.
    pub(in crate::features) fn last_agent_kind(&self) -> AiAgentKind {
        match self.panel.last_agent_kind {
            AiAgentKind::Codex if !self.settings.config.codex.enabled => AiAgentKind::Nyaterm,
            AiAgentKind::ClaudeCode if !self.settings.config.claude_code.enabled => {
                AiAgentKind::Nyaterm
            }
            ref kind => kind.clone(),
        }
    }

    pub(in crate::features) fn set_chat_run_mode(
        &mut self,
        mode: AiMode,
        kind: AiAgentKind,
    ) -> bool {
        if mode == AiMode::Agent
            && match kind {
                AiAgentKind::Codex => !self.settings.config.codex.enabled,
                AiAgentKind::ClaudeCode => !self.settings.config.claude_code.enabled,
                AiAgentKind::Nyaterm => false,
            }
        {
            return false;
        }
        let kind = if mode == AiMode::Ask {
            AiAgentKind::Nyaterm
        } else {
            self.panel.last_agent_kind = kind.clone();
            kind
        };
        self.settings.config.default_mode = mode.clone();
        self.settings.config.default_agent_kind = kind.clone();
        self.chat.run_mode = mode;
        self.chat.agent_kind = kind;
        true
    }

    pub(in crate::features) fn chat_prompt_draft(&self) -> &str {
        &self.chat.prompt_draft
    }

    pub(in crate::features) fn chat_request_prompt(&self) -> Option<String> {
        let prompt = self.chat.prompt_draft.trim();
        if prompt.is_empty() {
            return None;
        }
        Some(
            self.chat
                .quoted_text
                .as_deref()
                .map(str::trim)
                .filter(|quoted| !quoted.is_empty())
                .map(|quoted| format!("> {quoted}\n\n{prompt}"))
                .unwrap_or_else(|| prompt.to_string()),
        )
    }

    pub(in crate::features) fn reject_chat_start(
        &mut self,
        message: impl Into<String>,
        update_panel: bool,
    ) {
        self.chat.response_preview = message.into();
        if update_panel {
            self.panel.status = self.chat.response_preview.clone();
        }
    }

    pub(in crate::features) fn chat_prepared_request_cloned(&self) -> Option<AiPreparedRequest> {
        self.chat.prepared_request.clone()
    }

    pub(in crate::features) fn chat_target_session_ids(&self) -> &[String] {
        &self.chat.target_session_ids
    }

    pub(in crate::features) fn chat_mention_query(&self) -> &str {
        &self.chat.mention_query
    }

    pub(in crate::features) fn chat_targets_session(&self, session_id: &str) -> bool {
        self.chat
            .target_session_ids
            .iter()
            .any(|target_id| target_id == session_id)
    }

    pub(in crate::features) fn chat_mention_is_open(&self) -> bool {
        self.chat.mention_open
    }

    pub(in crate::features) fn chat_mention_index(&self) -> usize {
        self.chat.mention_index
    }

    pub(in crate::features) fn clamp_chat_mention_index(&mut self, len: usize) -> usize {
        if len == 0 {
            self.chat.mention_index = 0;
        } else {
            self.chat.mention_index = self.chat.mention_index.min(len - 1);
        }
        self.chat.mention_index
    }

    pub(in crate::features) fn set_chat_mention_index(&mut self, index: usize) {
        self.chat.mention_index = index;
    }

    pub(in crate::features) fn close_chat_mention(&mut self) {
        self.chat.close_mention();
    }

    pub(in crate::features) fn hide_chat_mention(&mut self) {
        self.chat.mention_open = false;
        self.chat.mention_query.clear();
    }

    pub(in crate::features) fn move_chat_mention_index(
        &mut self,
        candidate_count: usize,
        delta: isize,
    ) {
        if candidate_count == 0 {
            return;
        }
        self.chat.mention_index = if delta < 0 {
            (self.chat.mention_index + candidate_count - 1) % candidate_count
        } else {
            (self.chat.mention_index + 1) % candidate_count
        };
    }

    pub(in crate::features) fn set_chat_prompt_draft(&mut self, text: String) {
        self.chat.prompt_draft = text;
        self.chat.sync_mention_from_prompt();
    }

    pub(in crate::features) fn blur_chat_prompt(&mut self) {
        self.hide_chat_mention();
        self.chat.response_preview = "AI prompt blurred".to_string();
    }

    pub(in crate::features) fn remove_chat_target_session(&mut self, session_id: &str) {
        self.chat
            .target_session_ids
            .retain(|target_id| target_id != session_id);
        self.panel.status = if self.chat.target_session_ids.is_empty() {
            "AI target sessions cleared".to_string()
        } else {
            "AI target session removed".to_string()
        };
    }

    pub(in crate::features) fn select_chat_mention(
        &mut self,
        session_id: String,
        display_name: String,
    ) {
        if self
            .chat
            .target_session_ids
            .iter()
            .any(|target_id| target_id == &session_id)
        {
            self.chat
                .target_session_ids
                .retain(|target_id| target_id != &session_id);
        } else {
            self.chat.target_session_ids.push(session_id);
        }
        if let Some(at_index) = self.chat.prompt_draft.rfind('@') {
            let suffix = &self.chat.prompt_draft[at_index + 1..];
            if !suffix.chars().any(char::is_whitespace) {
                self.chat.prompt_draft.truncate(at_index);
            }
        }
        self.chat.close_mention();
        self.panel.status = format!("AI target session selected: {display_name}");
    }

    pub(in crate::features) fn begin_chat_job(&mut self) -> AiChatLaunch {
        self.visible_scope_epoch.fetch_add(1, Ordering::AcqRel);
        self.chat.job_id = self.chat.job_id.wrapping_add(1).max(1);
        let cancel = Arc::new(AtomicBool::new(false));
        self.chat.cancel = Some(cancel.clone());
        self.chat.response_phase = AiResponsePhase::Waiting;
        AiChatLaunch {
            job_id: self.chat.job_id,
            cancel,
            tx: self.chat.tx.clone(),
            session_id: self.chat.session_id.clone(),
        }
    }

    #[cfg(test)]
    pub(in crate::features) fn begin_chat_request(
        &mut self,
        request_prompt: String,
        mode: AiMode,
        source_label: Option<&str>,
    ) -> AiChatLaunch {
        self.begin_chat_request_with_language(request_prompt, mode, source_label, "en".into())
    }

    pub(in crate::features) fn begin_chat_request_with_language(
        &mut self,
        request_prompt: String,
        mode: AiMode,
        source_label: Option<&str>,
        language: String,
    ) -> AiChatLaunch {
        self.agent.task_language = Some(language);
        if let Some(previous) = self.agent.native.take() {
            previous.cancellation.cancel();
        }
        self.chat.provider_session = None;
        let launch = self.begin_chat_job();
        if mode == AiMode::Agent {
            self.agent.task_prompt = Some(request_prompt.clone());
            self.agent.conversation = vec![AiMessage {
                id: format!("agent-user-{}", uuid()),
                session_id: self.chat.session_id.clone(),
                role: AiMessageRole::User,
                content: request_prompt.clone(),
                created_at: nyaterm_core::now_rfc3339(),
                reasoning_content: None,
                command_cards: Vec::new(),
            }];
            self.agent.json_protocol = false;
            self.agent.step_index = 0;
            self.agent.steps.clear();
            self.agent.thought_expanded.clear();
            self.agent.output_expanded.clear();
            self.upsert_agent_step(
                0,
                AiAgentStepStatus::Planning,
                AiAgentStepKind::Planning,
                "Planning",
                truncate_preview(&request_prompt, 120),
            );
        } else {
            self.agent.task_prompt = None;
            self.agent.conversation.clear();
            self.agent.json_protocol = false;
            self.agent.step_index = 0;
            self.agent.loop_state = None;
            self.agent.steps.clear();
            self.agent.thought_expanded.clear();
            self.agent.output_expanded.clear();
        }
        self.chat.pending = true;
        self.chat.response_preview = if mode == AiMode::Agent {
            "Running AI Agent step...".to_string()
        } else {
            "Running AI request...".to_string()
        };
        self.chat.command_cards.clear();
        let now = nyaterm_core::now_rfc3339();
        let assistant_id = format!("assistant-{}", uuid());
        self.chat.messages.push(Arc::new(AiMessage {
            id: format!("user-{}", uuid()),
            session_id: self.chat.session_id.clone(),
            role: AiMessageRole::User,
            content: request_prompt,
            created_at: now.clone(),
            reasoning_content: None,
            command_cards: Vec::new(),
        }));
        self.chat.messages.push(Arc::new(AiMessage {
            id: assistant_id.clone(),
            session_id: self.chat.session_id.clone(),
            role: AiMessageRole::Assistant,
            content: String::new(),
            created_at: now,
            reasoning_content: None,
            command_cards: Vec::new(),
        }));
        self.chat.prompt_draft.clear();
        self.chat.quoted_text = None;
        self.chat.close_mention();
        self.chat.streaming_assistant_id = Some(assistant_id);
        self.panel.status = if mode == AiMode::Agent {
            "AI Agent step started".to_string()
        } else if let Some(source_label) = source_label {
            format!("AI file action started: {source_label}")
        } else {
            "AI Ask request started".to_string()
        };
        self.chat.prepared_request = None;
        launch
    }

    pub(in crate::features) fn cancel_all_chats_and_agents(&mut self) {
        // Switching/closing a terminal keeps its running conversation in an
        // inactive scope. Every worker must see cancellation before pool join.
        // Archived conversations cannot contain running jobs.
        let visible = self.active_scope_key.clone();
        let scopes = self.inactive_scopes.keys().cloned().collect::<Vec<_>>();
        self.cancel_chat_and_agent();
        for scope in scopes {
            self.switch_scope(&scope);
            self.cancel_chat_and_agent();
        }
        self.switch_scope(&visible);
    }

    pub(in crate::features) fn cancel_chat_and_agent(&mut self) {
        self.chat.native_task = None;
        if let Some(native) = &mut self.agent.native {
            native.cancellation.cancel();
            native.run.cancel();
        }
        if let Some(cancel) = self.chat.cancel.as_ref() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.chat.job_id = self.chat.job_id.wrapping_add(1).max(1);
        self.chat.pending = false;
        self.chat.response_phase = AiResponsePhase::Ended;
        self.chat.cancel = None;
        let cancelled_step = self
            .agent
            .loop_state
            .as_ref()
            .map(|state| state.step_index)
            .or_else(|| self.agent.steps.last().map(|step| step.step_index));
        self.agent.loop_state = None;
        self.agent.task_prompt = None;
        self.agent.conversation.clear();
        self.agent.json_protocol = false;
        self.chat.command_cards.clear();
        self.chat.response_preview = "AI request cancelled".to_string();
        if let Some(assistant_id) = self.chat.streaming_assistant_id.take()
            && let Some(message) = self
                .chat
                .messages
                .iter_mut()
                .rev()
                .find(|message| message.id == assistant_id)
            && message.content.trim().is_empty()
        {
            let message = Arc::make_mut(message);
            message.content = "AI request cancelled".to_string();
        }
        self.panel.status = "AI request cancelled".to_string();
        if let Some(step_index) = cancelled_step {
            self.upsert_agent_step(
                step_index,
                AiAgentStepStatus::Cancelled,
                AiAgentStepKind::Diagnostic,
                "Cancelled",
                "AI Agent request was cancelled",
            );
        }
    }

    pub(in crate::features) fn take_chat_event_receiver(
        &mut self,
    ) -> Option<UnboundedReceiver<AiChatWorkerEvent>> {
        self.chat.rx.take()
    }

    /// Whether a chat request is outstanding, so a worker event is still wanted.
    ///
    /// A cancelled or already-settled request leaves nothing to apply; the
    /// per-event `job_id` checks would reject it anyway, but dropping it here
    /// keeps that intent explicit.
    pub(in crate::features) fn chat_event_is_wanted(&self) -> bool {
        self.chat.pending || self.agent.loop_state.is_some()
    }

    pub(in crate::features) fn apply_chat_delta(
        &mut self,
        job_id: u64,
        text_delta: &str,
        reasoning_delta: Option<&str>,
    ) -> bool {
        if job_id != self.chat.job_id || !self.chat.pending {
            return false;
        }
        if self.chat.response_preview == "Running AI request..." {
            self.chat.response_preview.clear();
        }
        self.chat.response_preview.push_str(text_delta);
        self.chat.response_preview = truncate_preview(&self.chat.response_preview, 320);
        if let Some(assistant_id) = self.chat.streaming_assistant_id.as_deref()
            && let Some(message) = self
                .chat
                .messages
                .iter_mut()
                .rev()
                .find(|message| message.id == assistant_id)
        {
            let message = Arc::make_mut(message);
            message.content.push_str(text_delta);
            if let Some(delta) = reasoning_delta.filter(|delta| !delta.trim().is_empty()) {
                message
                    .reasoning_content
                    .get_or_insert_with(String::new)
                    .push_str(delta);
            }
        }
        self.refresh_response_phase();
        self.panel.status = if reasoning_delta.is_some_and(|delta| !delta.trim().is_empty()) {
            "AI stream receiving; reasoning captured".to_string()
        } else {
            "AI stream receiving".to_string()
        };
        true
    }

    pub(in crate::features) fn apply_agent_tool_delta(
        &mut self,
        job_id: u64,
        tool_name: Option<&str>,
        arguments_delta_len: usize,
    ) -> bool {
        if job_id != self.chat.job_id || !self.chat.pending {
            return false;
        }
        self.chat.response_phase = AiResponsePhase::ToolArguments;
        let tool_label = tool_name
            .filter(|name| !name.trim().is_empty())
            .unwrap_or("tool");
        self.panel.status = if arguments_delta_len == 0 {
            format!("AI Agent selected {tool_label}")
        } else {
            format!("AI Agent streaming {tool_label} arguments (+{arguments_delta_len} chars)")
        };
        self.upsert_agent_step(
            self.last_agent_step_index(),
            AiAgentStepStatus::Tool,
            AiAgentStepKind::ToolProgress,
            format!("Tool {tool_label}"),
            if arguments_delta_len == 0 {
                "Provider selected an Agent tool".to_string()
            } else {
                format!("Streaming arguments (+{arguments_delta_len} chars)")
            },
        );
        true
    }

    pub(in crate::features) fn apply_agent_protocol_fallback(&mut self, job_id: u64) -> bool {
        if !self.reset_agent_response_for_retry(job_id) {
            return false;
        }
        self.panel.status = "AI Agent retrying with JSON protocol".to_string();
        self.agent.json_protocol = true;
        true
    }

    pub(in crate::features) fn apply_agent_protocol_repair(&mut self, job_id: u64) -> bool {
        if !self.reset_agent_response_for_retry(job_id) {
            return false;
        }
        self.panel.status = "AI Agent correcting an invalid response (retry 1/1)".to_string();
        true
    }

    fn reset_agent_response_for_retry(&mut self, job_id: u64) -> bool {
        if job_id != self.chat.job_id || !self.chat.pending {
            return false;
        }
        if let Some(assistant_id) = self.chat.streaming_assistant_id.as_deref()
            && let Some(message) = self
                .chat
                .messages
                .iter_mut()
                .rev()
                .find(|message| message.id == assistant_id)
        {
            let message = Arc::make_mut(message);
            message.content.clear();
            message.reasoning_content = None;
        }
        self.chat.response_phase = AiResponsePhase::Waiting;
        self.chat.response_preview.clear();
        true
    }

    pub(in crate::features) fn finish_chat_job(
        &mut self,
        job_id: u64,
        session_id: String,
        result: Result<AiChatJobOutput, String>,
    ) -> Option<AiChatFinishEffect> {
        if job_id != self.chat.job_id || session_id != self.chat.session_id || !self.chat.pending {
            return None;
        }
        self.chat.pending = false;
        self.chat.response_phase = AiResponsePhase::Ended;
        self.chat.cancel = None;
        match result {
            Ok(output) => {
                if let Some(metadata) = &output.metadata {
                    let id = self
                        .chat
                        .streaming_assistant_id
                        .clone()
                        .unwrap_or_else(|| job_id.to_string());
                    self.chat.response_metadata.insert(id, metadata.clone());
                }
                let has_native_call = output.native_decision.is_some();
                if output.mode == AiMode::Agent && !has_native_call {
                    self.agent.conversation.push(AiMessage {
                        id: format!("agent-assistant-{}", uuid()),
                        session_id: self.chat.session_id.clone(),
                        role: AiMessageRole::Assistant,
                        content: output.text.clone(),
                        created_at: nyaterm_core::now_rfc3339(),
                        reasoning_content: output.reasoning.clone(),
                        command_cards: output.command_cards.clone(),
                    });
                }
                let command_count = output.command_cards.len();
                self.chat.response_preview = if output.text.trim().is_empty() {
                    "AI returned an empty response".to_string()
                } else {
                    truncate_preview(&output.text, 320)
                };
                let mode_label = if output.mode == AiMode::Agent {
                    "AI Agent"
                } else {
                    "AI Ask"
                };
                let mut status =
                    format!("{mode_label} completed; {command_count} command card(s) parsed");
                if output.reasoning.is_some() {
                    status.push_str("; reasoning captured");
                }
                if let Some(note) = output.approval_note.as_deref() {
                    status.push_str("; ");
                    status.push_str(note);
                }
                if output.mode == AiMode::Agent
                    && !has_native_call
                    && command_count > 0
                    && !output.auto_execute_first
                {
                    status.push_str("; awaiting command approval");
                }
                self.panel.status = status;
                if output.mode == AiMode::Agent && !has_native_call {
                    let (step_status, step_title) = if command_count == 0 && !has_native_call {
                        (AiAgentStepStatus::Completed, "Final Answer")
                    } else if output.auto_execute_first {
                        (AiAgentStepStatus::Running, "Auto Execute")
                    } else {
                        (AiAgentStepStatus::NeedsApproval, "Needs Approval")
                    };
                    self.upsert_agent_step(
                        self.last_agent_step_index(),
                        step_status,
                        if command_count == 0 && !has_native_call {
                            AiAgentStepKind::FinalAnswer
                        } else {
                            AiAgentStepKind::ToolProgress
                        },
                        step_title,
                        if command_count == 0 && !has_native_call {
                            output.text.clone()
                        } else {
                            truncate_preview(&output.text, 140)
                        },
                    );
                }
                if output.mode == AiMode::Agent && !has_native_call {
                    let source_message_id = self.chat.streaming_assistant_id.clone();
                    let step_index = self.last_agent_step_index();
                    if let Some(step) = self
                        .agent
                        .steps
                        .iter_mut()
                        .find(|step| step.step_index == step_index)
                    {
                        step.source_message_id = source_message_id;
                        step.command_card_id =
                            output.command_cards.first().map(|card| card.id.clone());
                        if let Some(card) = output.command_cards.first() {
                            step.command = Some(card.command.clone());
                            step.thought = output.reasoning.clone();
                            step.detail = output.approval_note.clone().unwrap_or_default();
                        }
                    }
                }
                self.chat.command_cards = output.command_cards.clone();
                if let Some(assistant_id) = self.chat.streaming_assistant_id.take()
                    && let Some(message) = self
                        .chat
                        .messages
                        .iter_mut()
                        .rev()
                        .find(|message| message.id == assistant_id)
                {
                    let message = Arc::make_mut(message);
                    if !output.text.trim().is_empty() {
                        message.content = output.text.clone();
                    } else if !output.command_cards.is_empty() {
                        message.content.clear();
                    } else if message.content.trim().is_empty() {
                        message.content = "AI returned an empty response".to_string();
                    }
                    message.reasoning_content = output.reasoning;
                    message.command_cards = output.command_cards;
                }
                if output.mode == AiMode::Agent && command_count == 0 && !has_native_call {
                    self.agent.loop_state = None;
                    self.agent.task_prompt = None;
                }
                Some(AiChatFinishEffect {
                    session_id,
                    succeeded: true,
                    // The submitted prompt was cleared when the request
                    // started. Text entered during the run is the next draft.
                    clear_prompt_input: false,
                    refresh_usage_counts: true,
                    auto_execute_first: output.auto_execute_first
                        && !self.chat.command_cards.is_empty(),
                })
            }
            Err(error) => {
                if let Some(native) = &mut self.agent.native {
                    native.run.status = nyaterm_core::ai::harness::AgentRunStatus::Failed;
                    native.cancellation.cancel();
                }
                self.chat.response_preview = format!("AI request failed: {error}");
                self.chat.command_cards.clear();
                self.panel.status = self.chat.response_preview.clone();
                if let Some(assistant_id) = self.chat.streaming_assistant_id.take()
                    && let Some(message) = self
                        .chat
                        .messages
                        .iter_mut()
                        .rev()
                        .find(|message| message.id == assistant_id)
                {
                    let message = Arc::make_mut(message);
                    message.content = format!("AI request failed: {error}");
                }
                if self.agent.task_prompt.is_some() {
                    self.upsert_agent_step(
                        self.last_agent_step_index(),
                        AiAgentStepStatus::Failed,
                        AiAgentStepKind::Diagnostic,
                        "Failed",
                        truncate_preview(&error, 140),
                    );
                    self.agent.task_prompt = None;
                    self.agent.conversation.clear();
                    self.agent.loop_state = None;
                }
                Some(AiChatFinishEffect {
                    session_id,
                    succeeded: false,
                    clear_prompt_input: false,
                    refresh_usage_counts: false,
                    auto_execute_first: false,
                })
            }
        }
    }

    pub(in crate::features) fn chat_prepared_request(&self) -> Option<&AiPreparedRequest> {
        self.chat.prepared_request.as_ref()
    }

    pub(in crate::features) fn chat_response_preview(&self) -> &str {
        &self.chat.response_preview
    }

    pub(in crate::features) fn set_chat_response_preview(&mut self, preview: impl Into<String>) {
        self.chat.response_preview = preview.into();
    }

    pub(in crate::features) fn chat_messages(&self) -> &[Arc<AiMessage>] {
        &self.chat.messages
    }

    pub(in crate::features) fn chat_snapshot_messages(&self) -> Arc<[Arc<AiMessage>]> {
        let streaming_id = self.chat_streaming_assistant_id();
        self.chat_messages()
            .iter()
            .map(|message| {
                if streaming_id == Some(message.id.as_str()) {
                    Arc::new((**message).clone())
                } else {
                    Arc::clone(message)
                }
            })
            .collect::<Vec<_>>()
            .into()
    }

    pub(in crate::features) fn chat_streaming_assistant_id(&self) -> Option<&str> {
        self.chat.streaming_assistant_id.as_deref()
    }

    pub(in crate::features) fn chat_command_cards(&self) -> &[AiCommandCard] {
        &self.chat.command_cards
    }

    pub(in crate::features) fn clear_chat_command_cards(&mut self) {
        self.chat.command_cards.clear();
    }

    pub(in crate::features) fn command_card(&self, index: usize) -> Option<AiCommandCard> {
        self.chat.command_cards.get(index).cloned()
    }

    pub(in crate::features) fn find_command_card(&self, card_id: &str) -> Option<AiCommandCard> {
        self.chat
            .command_cards
            .iter()
            .find(|card| card.id == card_id)
            .cloned()
            .or_else(|| {
                self.chat
                    .messages
                    .iter()
                    .flat_map(|message| message.command_cards.iter())
                    .find(|card| card.id == card_id)
                    .cloned()
            })
    }

    pub(in crate::features) fn chat_quote(&self) -> Option<&str> {
        self.chat.quoted_text.as_deref()
    }

    pub(in crate::features) fn quote_message(&mut self, text: String) -> bool {
        let value = text.trim().to_string();
        let quoted = !value.is_empty();
        if quoted {
            self.chat.quoted_text = Some(value);
            self.panel.status = "AI message quoted".to_string();
        } else {
            self.panel.status = "AI message is empty".to_string();
        }
        quoted
    }

    pub(in crate::features) fn prepare_external_request(
        &mut self,
        request: AiPreparedRequest,
        response_preview: impl Into<String>,
        status: impl Into<String>,
        focus: bool,
    ) {
        self.chat.prepared_request = Some(request);
        self.chat.response_preview = response_preview.into();
        self.panel.status = status.into();
        self.chat.focus_pending = focus;
        self.close_transient_menus();
    }

    pub(in crate::features) fn prepare_detected_error_request(
        &mut self,
        request: AiPreparedRequest,
        session_id: String,
    ) {
        self.chat.prepared_request = Some(request);
        if !self.chat.target_session_ids.contains(&session_id) {
            self.chat.target_session_ids.push(session_id);
        }
        self.close_transient_menus();
    }

    pub(in crate::features) fn history_is_open(&self) -> bool {
        self.history.open
    }

    pub(in crate::features) fn remember_history_focus(&mut self, focus: Option<FocusHandle>) {
        self.history.restore_focus = focus;
    }

    pub(in crate::features) fn take_history_focus(&mut self) -> Option<FocusHandle> {
        self.history.restore_focus.take()
    }

    pub(in crate::features) fn history_has_restore_focus(&self) -> bool {
        self.history.restore_focus.is_some()
    }

    pub(in crate::features) fn history_error(&self) -> Option<&str> {
        self.history.error.as_deref()
    }

    pub(in crate::features) fn history_load_is_current(
        &self,
        job_id: u64,
        scope: &str,
        source: &str,
    ) -> bool {
        self.history.job_id == job_id
            && self.history.pending
            && self.active_scope_key == scope
            && self.chat.session_id == source
            && !self.chat_or_agent_is_running()
    }

    pub(in crate::features) fn history_query(&self) -> &str {
        &self.history.query
    }

    pub(in crate::features) fn history_sessions(&self) -> &[AiSession] {
        &self.history.sessions
    }

    pub(in crate::features) fn history_session(&self, session_id: &str) -> Option<&AiSession> {
        self.history
            .sessions
            .iter()
            .find(|session| session.id == session_id)
    }

    pub(in crate::features) fn apply_loaded_session_kind(&mut self, kind: AiAgentKind) {
        self.chat.run_mode = if kind == AiAgentKind::Nyaterm {
            self.chat.run_mode.clone()
        } else {
            AiMode::Agent
        };
        self.chat.agent_kind = kind;
    }

    pub(in crate::features) fn replace_history_session(&mut self, session: AiSession) {
        if let Some(existing) = self
            .history
            .sessions
            .iter_mut()
            .find(|existing| existing.id == session.id)
        {
            *existing = session;
        } else {
            self.history.sessions.push(session);
        }
    }

    pub(in crate::features) fn history_is_pending(&self) -> bool {
        self.history.pending
    }

    pub(in crate::features) fn request_history_clear_confirm(&mut self) -> bool {
        if self.history.sessions.is_empty() {
            return false;
        }
        self.discovery.menu_open = false;
        self.panel.execution_menu_open = false;
        true
    }

    pub(in crate::features) fn confirm_history_clear(&mut self) -> bool {
        self.history.open = false;
        true
    }

    pub(in crate::features) fn close_history(&mut self) {
        self.history.open = false;
        self.history.query.clear();
    }

    pub(in crate::features) fn clear_history_query(&mut self) {
        self.history.query.clear();
    }

    pub(in crate::features) fn toggle_history(&mut self) -> bool {
        self.panel.execution_menu_open = false;
        self.history.open = !self.history.open;
        if self.history.open {
            self.discovery.menu_open = false;
        } else {
            self.history.query.clear();
        }
        self.history.open
    }

    pub(in crate::features) fn history_actions_are_disabled(&self) -> bool {
        self.history.sessions.is_empty() || self.history.pending || self.any_chat_or_agent_running()
    }

    pub(in crate::features) fn history_audit_write_lock(&self) -> Arc<Mutex<()>> {
        Arc::clone(&self.history.audit_write_lock)
    }

    pub(in crate::features) fn begin_history_operation(
        &mut self,
        status: impl Into<String>,
    ) -> Option<u64> {
        if self.history.pending {
            self.panel.status = "AI history operation already in progress".to_string();
            return None;
        }
        self.history.job_id = self.history.job_id.wrapping_add(1).max(1);
        self.history.pending = true;
        self.history.error = None;
        self.panel.status = status.into();
        Some(self.history.job_id)
    }

    pub(in crate::features) fn finish_history_session_list(
        &mut self,
        job_id: u64,
        result: Result<Vec<AiSession>, String>,
    ) -> bool {
        if self.history.job_id != job_id {
            return false;
        }
        self.history.pending = false;
        match result {
            Ok(sessions) => {
                self.history.sessions = sessions;
                self.panel.status = "AI history loaded".to_string();
            }
            Err(error) => {
                self.history.error = Some(error.clone());
                self.panel.status = format!("failed to load AI history: {error}");
            }
        }
        true
    }

    pub(in crate::features) fn finish_history_message_load(
        &mut self,
        job_id: u64,
        source_session_id: &str,
        target_session_id: String,
        result: Result<Vec<AiMessage>, String>,
        loaded_status: String,
    ) -> bool {
        if self.history.job_id != job_id {
            return false;
        }
        self.history.pending = false;
        if self.chat.session_id != source_session_id || self.chat_or_agent_is_running() {
            self.panel.status = "AI session load cancelled".to_string();
            return true;
        }
        match result {
            Ok(messages) => {
                if !self.restore_archived_session(&target_session_id) {
                    self.archive_current_session();
                }
                self.chat.session_id = target_session_id;
                self.chat.messages = messages.into_iter().map(Arc::new).collect();
                self.chat.streaming_assistant_id = None;
                self.chat.response_phase = AiResponsePhase::Ended;
                self.history.open = false;
                self.chat.quoted_text = None;
                self.chat.command_cards.clear();
                if let Some(last) = self
                    .chat
                    .messages
                    .iter()
                    .rev()
                    .find(|message| matches!(message.role, AiMessageRole::Assistant))
                {
                    self.chat.response_preview = truncate_preview(&last.content, 320);
                    self.chat.command_cards = last.command_cards.clone();
                } else {
                    self.chat.response_preview.clear();
                }
                self.panel.status = loaded_status;
            }
            Err(error) => {
                self.history.error = Some(error.clone());
                self.panel.status = format!("failed to load AI session: {error}");
            }
        }
        true
    }

    pub(in crate::features) fn finish_history_session_delete(
        &mut self,
        job_id: u64,
        session_id: &str,
        result: Result<(), String>,
    ) -> Option<bool> {
        if self.history.job_id != job_id {
            return None;
        }
        self.history.pending = false;
        match result {
            Ok(()) => {
                self.archived_sessions.remove(session_id);
                self.release_ai_session(session_id);
                if self.chat.session_id == session_id {
                    self.chat.messages.clear();
                    self.chat.command_cards.clear();
                    self.chat.streaming_assistant_id = None;
                    self.chat.response_phase = AiResponsePhase::Ended;
                    self.chat.quoted_text = None;
                    self.chat.session_id = format!("ai-session-{}", uuid());
                    self.chat.response_preview = "Ask mode ready".to_string();
                    self.agent = AiAgentState::fresh();
                }
                self.history
                    .sessions
                    .retain(|session| session.id != session_id);
                self.panel.status = "AI session deleted".to_string();
                Some(true)
            }
            Err(error) => {
                self.panel.status = format!("failed to delete AI session: {error}");
                Some(false)
            }
        }
    }

    pub(in crate::features) fn finish_history_clear(
        &mut self,
        job_id: u64,
        source_session_id: &str,
        result: Result<(), String>,
    ) -> Option<bool> {
        if self.history.job_id != job_id {
            return None;
        }
        self.history.pending = false;
        match result {
            Ok(()) => {
                self.history.sessions.clear();
                self.history.query.clear();
                self.archived_sessions.clear();
                for state in self.inactive_scopes.values_mut() {
                    state.chat.messages.clear();
                    state.chat.command_cards.clear();
                    state.chat.session_id = format!("ai-session-{}", uuid());
                    state.agent = AiAgentState::fresh();
                }
                if self.chat.session_id == source_session_id {
                    self.chat.messages.clear();
                    self.chat.command_cards.clear();
                    self.chat.streaming_assistant_id = None;
                    self.chat.response_phase = AiResponsePhase::Ended;
                    self.chat.quoted_text = None;
                    self.clear_detected_error();
                    self.chat.session_id = format!("ai-session-{}", uuid());
                    self.chat.response_preview = if self.chat.run_mode == AiMode::Agent {
                        "Agent mode ready".to_string()
                    } else {
                        "Ask mode ready".to_string()
                    };
                }
                self.panel.status = "AI history cleared".to_string();
                Some(true)
            }
            Err(error) => {
                self.panel.status = format!("failed to clear AI history: {error}");
                Some(false)
            }
        }
    }

    pub(in crate::features) fn set_history_query(&mut self, query: String) {
        self.history.query = query;
    }

    pub(in crate::features) fn begin_history_usage_count_job(&mut self) -> u64 {
        self.history.usage_count_job_id = self.history.usage_count_job_id.wrapping_add(1).max(1);
        self.history.usage_count_job_id
    }

    pub(in crate::features) fn finish_history_usage_counts(
        &mut self,
        job_id: u64,
        result: Result<(usize, usize, usize), String>,
    ) -> bool {
        if self.history.usage_count_job_id != job_id {
            return false;
        }
        let Ok((sessions, messages, audits)) = result else {
            return false;
        };
        self.history.session_count = sessions;
        self.history.message_count = messages;
        self.history.audit_count = audits;
        true
    }

    #[cfg(test)]
    pub(in crate::features) fn discovery_is_pending(&self) -> bool {
        self.discovery.pending
    }

    pub(in crate::features) fn discovery_menu_is_open(&self) -> bool {
        self.discovery.menu_open
    }

    pub(in crate::features) fn discovery_query(&self) -> &str {
        &self.discovery.query
    }

    pub(in crate::features) fn discovery_index(&self) -> usize {
        self.discovery.index
    }

    pub(in crate::features) fn clamp_discovery_index(&mut self, len: usize) -> usize {
        if len == 0 {
            self.discovery.index = 0;
        } else {
            self.discovery.index = self.discovery.index.min(len - 1);
        }
        self.discovery.index
    }

    pub(in crate::features) fn set_discovery_index(&mut self, index: usize) {
        self.discovery.index = index;
    }

    pub(in crate::features) fn toggle_discovery_menu(&mut self, selected_index: usize) -> bool {
        self.discovery.menu_open = !self.discovery.menu_open;
        if self.discovery.menu_open {
            self.discovery.index = selected_index;
            self.history.open = false;
            self.panel.execution_menu_open = false;
        } else {
            self.discovery.query.clear();
            self.discovery.index = 0;
        }
        self.discovery.menu_open
    }

    pub(in crate::features) fn close_discovery_menu(&mut self) {
        self.discovery.menu_open = false;
        self.discovery.query.clear();
        self.discovery.index = 0;
    }

    #[cfg(test)]
    pub(in crate::features) fn begin_discovery_job(
        &mut self,
    ) -> Option<UnboundedSender<AiDiscoveryJobResult>> {
        if self.discovery.pending {
            self.panel.status = "AI model discovery already running".to_string();
            return None;
        }
        self.discovery.pending = true;
        self.panel.status = "Discovering AI models...".to_string();
        Some(self.discovery.tx.clone())
    }

    pub(in crate::features) fn take_discovery_event_receiver(
        &mut self,
    ) -> Option<UnboundedReceiver<AiDiscoveryJobResult>> {
        self.discovery.rx.take()
    }

    /// A discovery reply settles the job it belongs to.
    pub(in crate::features) fn note_discovery_event_delivered(&mut self) {
        self.discovery.pending = false;
    }

    pub(in crate::features) fn set_discovery_query(&mut self, query: String) {
        self.discovery.query = query;
        self.discovery.index = 0;
    }

    /// Returns whether the text field must also be cleared.
    pub(in crate::features) fn escape_discovery_search(&mut self, selected_index: usize) -> bool {
        if self.discovery.query.is_empty() {
            self.discovery.menu_open = false;
            false
        } else {
            self.discovery.query.clear();
            self.discovery.index = selected_index;
            true
        }
    }

    pub(in crate::features) fn move_discovery_index(&mut self, choice_count: usize, delta: isize) {
        if choice_count == 0 {
            return;
        }
        self.discovery.index = if delta < 0 {
            (self.discovery.index + choice_count - 1) % choice_count
        } else {
            (self.discovery.index + 1) % choice_count
        };
    }

    pub(in crate::features) fn agent_steps(&self) -> &[AiAgentStepView] {
        &self.agent.steps
    }

    pub(in crate::features) fn toggle_agent_thought_expanded(&mut self, step_index: u16) {
        if !self.agent.thought_expanded.remove(&step_index) {
            self.agent.thought_expanded.insert(step_index);
        }
    }

    pub(in crate::features) fn toggle_agent_output_expanded(&mut self, step_index: u16) {
        if !self.agent.output_expanded.remove(&step_index) {
            self.agent.output_expanded.insert(step_index);
        }
    }

    pub(in crate::features) fn agent_task_prompt_or_preview(&self) -> String {
        self.agent
            .task_prompt
            .clone()
            .unwrap_or_else(|| self.chat.response_preview.clone())
    }

    pub(in crate::features) fn begin_agent_step(
        &mut self,
        max_steps: u16,
    ) -> Result<(String, u16), String> {
        if let Some(native) = &self.agent.native {
            return Ok((
                native.run.objective.clone(),
                native.run.used_steps.saturating_sub(1),
            ));
        }
        let step_index = self.agent.step_index;
        if step_index.saturating_add(1) >= max_steps {
            self.agent.loop_state = None;
            self.agent.task_prompt = None;
            self.agent.conversation.clear();
            self.chat.command_cards.clear();
            return Err(format!(
                "AI Agent reached max step limit ({max_steps}); review terminal output"
            ));
        }
        self.agent.step_index = self.agent.step_index.saturating_add(1);
        Ok((self.agent_task_prompt_or_preview(), step_index))
    }

    pub(in crate::features) fn set_agent_loop(&mut self, state: AiAgentLoopState) {
        if let Some(card_id) = state.command_card_id.as_deref() {
            self.associate_agent_command(state.step_index, card_id);
        }
        self.agent.loop_state = Some(state);
    }

    pub(in crate::features) fn take_agent_loop_for_session(
        &mut self,
        session_id: &str,
    ) -> Option<AiAgentLoopState> {
        if !self
            .agent
            .loop_state
            .as_ref()
            .is_some_and(|state| state.terminal_session_id == session_id)
        {
            return None;
        }
        let state = self.agent.loop_state.take()?;
        Some(state)
    }

    pub(in crate::features) fn begin_agent_continuation(
        &mut self,
        state: &AiAgentLoopState,
        observation_message: &str,
    ) -> Option<AiChatLaunch> {
        // A completion from another conversation must never create a response
        // or reclaim the active conversation's streaming message.
        if state.ai_session_id != self.chat.session_id {
            return None;
        }
        if self.chat.pending {
            self.agent.loop_state = Some(state.clone());
            return None;
        }
        let mut launch = self.begin_chat_job();
        self.agent.conversation.push(AiMessage {
            id: format!("agent-observation-{}", uuid()),
            session_id: state.ai_session_id.clone(),
            role: AiMessageRole::User,
            content: observation_message.to_string(),
            created_at: nyaterm_core::now_rfc3339(),
            reasoning_content: None,
            command_cards: Vec::new(),
        });
        launch.session_id = state.ai_session_id.clone();
        let assistant_id = format!("assistant-{}", uuid());
        self.chat.messages.push(Arc::new(AiMessage {
            id: assistant_id.clone(),
            session_id: state.ai_session_id.clone(),
            role: AiMessageRole::Assistant,
            content: String::new(),
            created_at: nyaterm_core::now_rfc3339(),
            reasoning_content: None,
            command_cards: Vec::new(),
        }));
        self.chat.streaming_assistant_id = Some(assistant_id);
        self.chat.pending = true;
        self.chat.response_preview = format!(
            "Running AI Agent continuation step {}/{}...",
            state.step_index + 2,
            state.max_steps
        );
        self.chat.command_cards.clear();
        self.panel.status = self.chat.response_preview.clone();
        self.upsert_agent_step(
            state.step_index.saturating_add(1),
            AiAgentStepStatus::Planning,
            AiAgentStepKind::Planning,
            "Planning",
            "Continuing from the latest command observation",
        );
        Some(launch)
    }

    pub(in crate::features) fn agent_task_language(&self) -> &str {
        self.agent
            .native
            .as_ref()
            .map(|native| native.request.options.language.as_str())
            .or(self.agent.task_language.as_deref())
            .unwrap_or("en")
    }

    pub(in crate::features) fn agent_conversation_snapshot(&self) -> Vec<AiMessage> {
        self.agent.conversation.clone()
    }

    pub(in crate::features) fn agent_uses_json_protocol(&self) -> bool {
        self.agent.json_protocol
    }

    pub(in crate::features) fn agent_thought_is_expanded(&self, step_index: u16) -> bool {
        self.agent.thought_expanded.contains(&step_index)
    }

    pub(in crate::features) fn agent_output_is_expanded(&self, step_index: u16) -> bool {
        self.agent.output_expanded.contains(&step_index)
    }

    pub(in crate::features) fn request_agent_auto_confirm(&mut self) {
        self.close_transient_menus();
    }

    pub(in crate::features) fn confirm_agent_auto_execution(&mut self) -> bool {
        self.settings.config.agent_command_execution_mode = AgentCommandExecutionMode::Auto;
        self.panel.status = "Agent execution mode: auto".to_string();
        true
    }

    pub(in crate::features) fn last_agent_step_index(&self) -> u16 {
        if self.agent.native.is_some() {
            return self.agent.step_index;
        }
        self.agent
            .steps
            .last()
            .map(|step| step.step_index)
            .unwrap_or(0)
    }

    pub(in crate::features) fn agent_loop_snapshot(&self) -> Option<AiAgentLoopState> {
        self.agent.loop_state.clone()
    }

    pub(in crate::features) fn request_panel_refresh(&mut self) -> bool {
        if self.panel.panel_refresh_requested {
            return false;
        }
        self.panel.panel_refresh_requested = true;
        true
    }

    pub(in crate::features) fn take_panel_refresh_request(&mut self) -> bool {
        std::mem::take(&mut self.panel.panel_refresh_requested)
    }

    pub(in crate::features) fn clear_panel_refresh_request(&mut self) {
        self.panel.panel_refresh_requested = false;
        self.panel.stream_refresh_generation = self.panel.stream_refresh_generation.wrapping_add(1);
        self.panel.stream_refresh_pending = false;
    }

    pub(in crate::features) fn request_stream_refresh(&mut self) -> Option<u64> {
        if self.panel.stream_refresh_pending {
            return None;
        }
        self.panel.stream_refresh_pending = true;
        Some(self.panel.stream_refresh_generation)
    }

    pub(in crate::features) fn take_stream_refresh(&mut self, generation: u64) -> bool {
        if generation != self.panel.stream_refresh_generation {
            return false;
        }
        std::mem::take(&mut self.panel.stream_refresh_pending)
    }

    pub(in crate::features) fn reset_agent_runtime(&mut self) {
        self.agent.loop_state = None;
    }

    pub(in crate::features) fn panel_status(&self) -> &str {
        &self.panel.status
    }

    pub(in crate::features) fn set_panel_status(&mut self, status: impl Into<String>) {
        self.panel.status = status.into();
    }

    pub(in crate::features) fn apply_settings_input(&mut self, field: AiInputField, text: String) {
        if matches!(
            field,
            AiInputField::CodexExecutable
                | AiInputField::CodexConfigDirectory
                | AiInputField::ClaudeExecutable
                | AiInputField::ClaudeConfigDirectory
        ) {
            self.agent_management.invalidate();
        }
        self.panel.focused_field = field;
        match field {
            AiInputField::Model => self.settings.model_draft = text,
            AiInputField::BaseUrl => self.settings.base_url_draft = text,
            AiInputField::ApiKey => self.settings.secret_draft = text.into(),
            AiInputField::RequestUserAgent => self.settings.config.request_user_agent = text,
            AiInputField::ProxyHost => self.settings.config.proxy.host = text,
            AiInputField::ProxyPort => self.settings.config.proxy.port = text.parse().unwrap_or(0),
            AiInputField::ProxyUsername => self.settings.config.proxy.username = non_empty(text),
            AiInputField::ProxyPassword => self.settings.config.proxy.password = Some(text.into()),
            AiInputField::ProxyBypass => self.settings.config.proxy.no_proxy = text,
            AiInputField::CodexExecutable => {
                self.settings.config.codex.executable_path = non_empty(text)
            }
            AiInputField::CodexDefaultModel => {
                self.settings.config.codex.default_model = non_empty(text)
            }
            AiInputField::CodexConfigDirectory => {
                self.settings.config.codex.config_directory = non_empty(text)
            }
            AiInputField::ClaudeExecutable => {
                self.settings.config.claude_code.executable_path = non_empty(text)
            }
            AiInputField::ClaudeDefaultModel => {
                self.settings.config.claude_code.default_model = non_empty(text)
            }
            AiInputField::ClaudeConfigDirectory => {
                self.settings.config.claude_code.config_directory = non_empty(text)
            }
        }
        self.panel.status = "AI settings edited".to_string();
    }

    pub(in crate::features) fn panel_execution_menu_is_open(&self) -> bool {
        self.panel.execution_menu_open
    }

    pub(in crate::features) fn composer_min_height(&self) -> f32 {
        self.panel.composer_min_height
    }

    pub(in crate::features) fn composer_is_resizing(&self) -> bool {
        self.panel.composer_resize.is_some()
    }

    pub(in crate::features) fn start_composer_resize(
        &mut self,
        start_y: Pixels,
        start_height: Pixels,
    ) {
        self.panel.start_composer_resize(start_y, start_height);
    }

    pub(in crate::features) fn update_composer_resize(
        &mut self,
        current_y: Pixels,
        natural_height: Pixels,
        maximum_height: Pixels,
    ) -> Option<f32> {
        self.panel
            .update_composer_resize(current_y, natural_height, maximum_height)
    }

    pub(in crate::features) fn finish_composer_resize(&mut self) -> bool {
        self.panel.finish_composer_resize()
    }

    pub(in crate::features) fn toggle_execution_menu(&mut self) -> bool {
        self.history.open = false;
        self.history.query.clear();
        self.panel.execution_menu_open = !self.panel.execution_menu_open;
        if self.panel.execution_menu_open {
            self.discovery.menu_open = false;
        }
        self.panel.execution_menu_open
    }

    pub(in crate::features) fn close_execution_menu(&mut self) {
        self.panel.execution_menu_open = false;
    }

    pub(in crate::features) fn panel_detected_error(&self) -> Option<&AiDetectedErrorState> {
        self.panel.detected_error.as_ref()
    }

    pub(in crate::features) fn dismiss_detected_error(&mut self) {
        self.panel.dismiss_detected_error();
    }

    pub(in crate::features) fn clear_detected_error(&mut self) {
        self.panel.detected_error = None;
    }

    pub(in crate::features) fn note_detected_error(
        &mut self,
        session_id: String,
        output: String,
        now: Instant,
    ) -> bool {
        if self
            .panel
            .error_notice_at
            .get(&session_id)
            .is_some_and(|last| now.duration_since(*last) < std::time::Duration::from_secs(30))
        {
            return false;
        }
        self.panel.error_notice_at.insert(session_id.clone(), now);
        self.panel.detected_error = Some(AiDetectedErrorState { session_id, output });
        self.panel.status = "terminal error detected".to_string();
        true
    }

    pub(in crate::features) fn close_transient_menus(&mut self) {
        self.history.open = false;
        self.discovery.menu_open = false;
        self.panel.execution_menu_open = false;
    }

    pub(in crate::features) fn transient_menus_are_open(&self) -> bool {
        self.history.open || self.discovery.menu_open || self.panel.execution_menu_open
    }
}

impl AiChatState {
    /// Tracks the trailing `@mention` the composer is currently completing.
    ///
    /// Only a trailing run with no whitespace counts, so the picker closes as
    /// soon as the user types past the mention. The rules are unchanged.
    fn sync_mention_from_prompt(&mut self) {
        let Some(at_index) = self.prompt_draft.rfind('@') else {
            self.close_mention();
            return;
        };
        let query = &self.prompt_draft[at_index + 1..];
        if query.chars().any(char::is_whitespace) {
            self.close_mention();
            return;
        }
        if self.mention_query != query {
            self.mention_query = query.to_string();
            self.mention_index = 0;
        }
        self.mention_open = true;
    }

    fn close_mention(&mut self) {
        self.mention_open = false;
        self.mention_query.clear();
        self.mention_index = 0;
    }
}

impl AiPanelState {
    fn start_composer_resize(&mut self, start_y: Pixels, start_height: Pixels) {
        self.composer_resize = Some(AiComposerResize {
            start_y,
            start_height,
            start_floor: self.composer_min_height,
        });
    }

    fn update_composer_resize(
        &mut self,
        current_y: Pixels,
        natural_height: Pixels,
        maximum_height: Pixels,
    ) -> Option<f32> {
        let state = self.composer_resize?;
        let delta = f32::from(state.start_y - current_y);
        let maximum = maximum_height
            .max(gpui::px(0.))
            .min(super::panel::layout::COMPOSER_HEIGHT_MAX);
        let floor = (f32::from(state.start_height) + delta).min(f32::from(maximum));
        // A click or an upward drag already at the ceiling must not overwrite
        // a taller runtime preference hidden by the current panel size.
        self.composer_min_height =
            if delta == 0. || (delta > 0. && floor <= f32::from(state.start_height)) {
                state.start_floor
            } else if delta < 0. && floor <= f32::from(natural_height.min(maximum)) {
                0.
            } else {
                floor
            };
        Some(self.composer_min_height)
    }

    fn finish_composer_resize(&mut self) -> bool {
        self.composer_resize.take().is_some()
    }

    pub(in crate::features) fn dismiss_detected_error(&mut self) {
        self.detected_error = None;
        self.status = "terminal error notice dismissed".to_string();
    }
}

/// Transitions that span more than one AI concern.
impl AiFeatureState {
    pub(in crate::features) fn shutdown_agent_management_worker(
        &mut self,
    ) -> Option<std::thread::JoinHandle<()>> {
        self.agent_management.begin_shutdown()
    }

    pub(in crate::features) fn agent_management_view(&self) -> &AgentManagementView {
        self.agent_management.view()
    }

    pub(in crate::features) fn set_agent_management_error(&mut self, error: String) {
        self.agent_management.set_error(error);
    }

    pub(in crate::features) fn submit_agent_command(&mut self, command: AgentCommand) -> bool {
        self.agent_management.submit(command)
    }

    pub(in crate::features) fn take_agent_events(
        &mut self,
    ) -> Option<UnboundedReceiver<AgentJobEvent>> {
        self.agent_management.take_events()
    }

    pub(in crate::features) fn apply_agent_event(
        &mut self,
        event: AgentJobEvent,
    ) -> Option<String> {
        self.agent_management.apply_job(event)
    }

    pub(in crate::features) fn clear_quote(&mut self) {
        self.chat.quoted_text = None;
        self.panel.status = "AI quote cleared".to_string();
    }

    /// Resets every per-conversation concern and mints a new session id.
    ///
    /// Provider settings are deliberately untouched; the response preview is
    /// seeded from the configured default mode exactly as before.
    pub(in crate::features) fn start_new_chat(&mut self) {
        self.visible_scope_epoch.fetch_add(1, Ordering::AcqRel);
        self.archive_current_session();
        self.chat.prompt_draft.clear();
        self.chat.target_session_ids.clear();

        self.chat.quoted_text = None;
        self.chat.close_mention();
        self.chat.response_preview = if self.chat.run_mode == AiMode::Agent {
            "Agent mode ready".to_string()
        } else {
            "Ask mode ready".to_string()
        };
        self.chat.command_cards.clear();
        self.chat.messages.clear();
        self.chat.thought_expanded.clear();
        self.chat.command_details_expanded.clear();
        self.chat.command_scripts_expanded.clear();
        self.chat.agent_history_expanded = false;
        self.chat.execution_groups_expanded.clear();
        self.chat.response_phase = AiResponsePhase::Ended;
        self.chat.streaming_assistant_id = None;
        self.chat.prepared_request = None;
        self.chat.session_id = format!("ai-session-{}", uuid());

        self.agent.task_prompt = None;
        self.agent.conversation.clear();
        self.agent.json_protocol = false;
        self.agent.step_index = 0;
        self.agent.loop_state = None;
        self.agent.steps.clear();
        self.agent.thought_expanded.clear();
        self.agent.output_expanded.clear();

        self.history.open = false;
        self.history.query.clear();

        self.discovery.menu_open = false;
        self.discovery.query.clear();
        self.discovery.index = 0;

        self.panel.detected_error = None;
        self.panel.execution_menu_open = false;
        self.panel.status = "new AI chat".to_string();
    }
}

#[cfg(test)]
mod tests;
