use futures::StreamExt as _;
use gpui::{Context, KeyDownEvent};
use nyaterm_core::{
    AiAction, AiAgentKind, AiBackendKind, AiChatRequest, AiContext, AiMode, AiSessionScope,
    AiSessionScopeType, AiTargetContext, AiTerminalTarget,
};
use nyaterm_transport::SessionInfo;

use crate::features::{
    NyaTermApp, formatting::compact_id, formatting::recent_terminal_output,
    formatting::session_kind_label, runtime_jobs::AiChatJobResult, runtime_jobs::AiChatWorkerEvent,
};
use crate::models::SessionLaunchConfig;

use super::super::super::ai_jobs::{AiJobRunOptions, run_ai_ask_job};

impl NyaTermApp {
    pub(in crate::features) fn cancel_ai_chat(&mut self, cx: &mut Context<Self>) {
        let before = self.ai_header_presentation();
        if self
            .ai
            .native_pending_call()
            .is_some_and(|call| call.tool == nyaterm_core::ai::harness::AgentTool::RequestUserInput)
        {
            self.record_native_control_audit("request_user_input", false, cx);
        }
        if let Some(view) = self.ai.native_run_view() {
            self.forget_text_inputs(&format!("ai.agent-answer.{}.", view.run_id));
        }
        self.mcp
            .capabilities
            .terminal_executions
            .forget_owner(&format!("ai-session:{}", self.ai.chat_session_id()));
        let native_owner = self.ai.native_owner();
        self.ai.cancel_chat_and_agent();
        if let Some(owner) = native_owner {
            self.clear_capability_owner(&owner, cx);
        }
        self.poll_terminal_executions(cx);
        self.sync_session_event_bridge_policy();
        self.settings
            .set_store_message(self.ai.panel_status().to_string());
        self.request_settings_panel_refresh(cx);
        self.defer_ai_panel_snapshot_flush(cx);
        self.notify_root_if_ai_header_changed(before, cx);
    }

    pub(in crate::features) fn start_ai_ask(&mut self, cx: &mut Context<Self>) {
        if self.ai.history_is_pending() {
            return;
        }
        self.sync_ai_active_scope(cx);
        if self.ai.chat_or_agent_is_running() {
            self.ai
                .reject_chat_start("AI request already running", false);
            self.defer_ai_panel_snapshot_flush(cx);
            return;
        }
        if self.ai.agent_loop_snapshot().is_some() {
            self.ai
                .reject_chat_start("AI Agent step already running", true);
            self.defer_ai_panel_snapshot_flush(cx);
            return;
        }
        let Some(request_prompt) = self.ai.chat_request_prompt() else {
            self.ai.reject_chat_start("Enter a prompt first", false);
            self.defer_ai_panel_snapshot_flush(cx);
            return;
        };
        if !self.ai.settings_enabled() {
            self.ai.reject_chat_start("AI assistant is disabled", false);
            self.defer_ai_panel_snapshot_flush(cx);
            return;
        }
        let settings = self.ai.settings_config_cloned();
        let mode = self.ai.chat_run_mode();
        let agent_kind = if mode == AiMode::Agent {
            self.ai.chat_agent_kind()
        } else {
            AiAgentKind::Nyaterm
        };
        let model_id = if agent_kind == AiAgentKind::Nyaterm {
            let Some(model_id) = self.ai_selected_model_id() else {
                self.ai
                    .reject_chat_start("Enable an AI model before sending", true);
                self.defer_ai_panel_snapshot_flush(cx);
                return;
            };
            Some(model_id)
        } else {
            None
        };
        let target_session_ids = self.ai_effective_target_session_ids();
        let target_session_id = target_session_ids.first().cloned();
        if mode == AiMode::Agent && target_session_id.is_none() {
            self.ai
                .reject_chat_start("Start a terminal session before running Agent mode", true);
            self.defer_ai_panel_snapshot_flush(cx);
            return;
        }
        let prepared_request = self.ai.chat_prepared_request_cloned();
        let action = prepared_request
            .as_ref()
            .map(|request| request.action.clone())
            .unwrap_or(AiAction::GenerateCommand);
        let context = prepared_request
            .as_ref()
            .map(|request| request.context.clone())
            .unwrap_or_else(|| self.ai_terminal_context_for_sessions(&target_session_ids));
        let targets = self.ai_terminal_targets_for_sessions(&target_session_ids);
        let target_contexts = targets
            .iter()
            .cloned()
            .map(|target| AiTargetContext {
                context: self.ai_terminal_context_for_session(Some(&target.terminal_session_id)),
                target: Some(target),
            })
            .collect::<Vec<_>>();
        let owner_scope = self.ai_current_owner_scope();
        let connection_id = self
            .session
            .active_id()
            .and_then(|id| self.session.metadata(id))
            .and_then(|metadata| metadata.source_connection_id.clone());

        let source_label = prepared_request
            .as_ref()
            .map(|request| request.source_label.clone());
        let session_id = self.ai.chat_session_id().to_string();
        let mut request = AiChatRequest {
            stream_id: None,
            session_id: Some(session_id.clone()),
            connection_id,
            terminal_session_id: target_session_id.clone(),
            owner_scope,
            targets,
            target_contexts,
            mode: mode.clone(),
            agent_kind,
            permission_mode: settings.external_agent_permission_mode.clone(),
            model_id,
            model_name: None,
            default_target_session_id: target_session_id.clone(),
            existing_external_session_id: None,
            attachments: Vec::new(),
            action,
            user_input: request_prompt.clone(),
            context,
            options: nyaterm_core::AiRequestOptions {
                language: crate::i18n::normalize_locale(&self.settings.summary().language)
                    .into_owned(),
                ..Default::default()
            },
        };
        let external_agent = matches!(
            request.agent_kind,
            AiAgentKind::Codex | AiAgentKind::ClaudeCode
        );
        let tool_integration_enabled = match request.agent_kind {
            AiAgentKind::Codex => {
                settings.codex.tool_integration_mode.as_deref() == Some("nyaterm_mcp")
            }
            AiAgentKind::ClaudeCode => {
                settings.claude_code.tool_integration_mode.as_deref() == Some("nyaterm_mcp")
            }
            AiAgentKind::Nyaterm => true,
        };
        if external_agent && !tool_integration_enabled {
            self.ai.reject_chat_start(
                "External agents require strict NyaTerm MCP tool integration",
                true,
            );
            self.defer_ai_panel_snapshot_flush(cx);
            return;
        }
        let mcp_credential = if external_agent {
            match self.create_ephemeral_mcp_credential(
                target_session_ids.clone(),
                target_session_id.clone(),
                request.permission_mode.clone(),
                crate::features::capability_runtime::terminal_execution::TerminalExecutionPolicy {
                    background: settings.agent_background_execution_enabled,
                    max_lines: settings.terminal_output_lines,
                },
            ) {
                Ok(credential) => Some(credential),
                Err(error) => {
                    self.ai.reject_chat_start(
                        format!("Could not create external Agent MCP credential: {error}"),
                        true,
                    );
                    self.defer_ai_panel_snapshot_flush(cx);
                    return;
                }
            }
        } else {
            None
        };
        let before = self.ai_header_presentation();
        let store = self.store_blocking_client();
        let launch = self.ai.begin_chat_request_with_language(
            request_prompt,
            mode.clone(),
            source_label.as_deref(),
            request.options.language.clone(),
        );
        self.reset_text_input("ai.chat.prompt", "", cx);
        let mut agent_history = None;
        if request.mode == AiMode::Agent && request.agent_kind == AiAgentKind::Nyaterm {
            self.ai.begin_native_run(request.clone());
            request.options.agent_context = self.ai.native_request_context();
            agent_history = self.ai.native_initial_history();
        }
        if request.agent_kind == AiAgentKind::Nyaterm {
            self.launch_native_chat(request, launch, agent_history, true, cx);
            self.defer_ai_panel_snapshot_flush(cx);
            self.notify_root_if_ai_header_changed(before, cx);
            return;
        }
        let job_id = launch.job_id;
        let cancel = launch.cancel;
        let tx = launch.tx;
        let rejected_tx = tx.clone();
        let rejected_session_id = session_id.clone();
        if let Err(error) = self
            .blocking_jobs
            .submit_detached("ai-chat", move |scheduler_cancel| {
                let result = if scheduler_cancel.is_cancelled() {
                    Err("AI chat cancelled".to_string())
                } else {
                    run_ai_ask_job(
                        store,
                        settings,
                        request,
                        AiJobRunOptions {
                            mcp_credential,
                            stream_tx: Some(tx.clone()),
                            cancel,
                            job_id,
                            agent_history,
                            persist_user_message: true,
                        },
                    )
                };
                let _ = tx.unbounded_send(AiChatWorkerEvent::Finished(Box::new(AiChatJobResult {
                    job_id,
                    session_id,
                    result,
                })));
            })
        {
            let _ = rejected_tx.unbounded_send(AiChatWorkerEvent::Finished(Box::new(
                AiChatJobResult {
                    job_id,
                    session_id: rejected_session_id,
                    result: Err(error.to_string()),
                },
            )));
        }
        self.defer_ai_panel_snapshot_flush(cx);
        self.notify_root_if_ai_header_changed(before, cx);
    }

    pub(in crate::features) fn ai_terminal_context(&self) -> AiContext {
        self.ai_terminal_context_for_session(self.session.active_id())
    }

    pub(in crate::features) fn ai_selected_model_id(&self) -> Option<String> {
        self.ai
            .settings_config()
            .models
            .iter()
            .find(|model| {
                model.enabled
                    && model.backend == AiBackendKind::Genai
                    && self.ai.settings_config().default_model_id.as_deref()
                        == Some(model.id.as_str())
            })
            .or_else(|| {
                self.ai
                    .settings_config()
                    .models
                    .iter()
                    .find(|model| model.enabled && model.backend == AiBackendKind::Genai)
            })
            .map(|model| model.id.clone())
    }

    pub(in crate::features) fn ai_enabled_models(&self) -> Vec<nyaterm_core::AiModelConfigItem> {
        self.ai
            .settings_config()
            .models
            .iter()
            .filter(|model| model.enabled && model.backend == AiBackendKind::Genai)
            .cloned()
            .collect()
    }

    pub(in crate::features) fn ai_model_provider_label(
        &self,
        model: &nyaterm_core::AiModelConfigItem,
    ) -> String {
        model
            .credential_id
            .as_ref()
            .and_then(|credential_id| {
                self.ai
                    .settings_config()
                    .provider_credentials
                    .iter()
                    .find(|credential| &credential.id == credential_id)
                    .map(|credential| credential.name.clone())
            })
            .or_else(|| model.provider_kind.as_ref().map(|kind| format!("{kind:?}")))
            .unwrap_or_else(|| "model".to_string())
    }

    pub(in crate::features) fn ai_filtered_model_choices(
        &self,
    ) -> Vec<(nyaterm_core::AiModelConfigItem, String)> {
        let query = self.ai.discovery_query().trim().to_ascii_lowercase();
        self.ai_enabled_models()
            .into_iter()
            .filter_map(|model| {
                let provider_label = self.ai_model_provider_label(&model);
                let search_value =
                    format!("{} {} {}", model.name, provider_label, model.id).to_ascii_lowercase();
                (query.is_empty() || search_value.contains(&query))
                    .then_some((model, provider_label))
            })
            .collect()
    }

    pub(in crate::features) fn ai_selected_model_index(&self) -> usize {
        let Some(selected_model_id) = self.ai_selected_model_id() else {
            return 0;
        };
        self.ai_filtered_model_choices()
            .iter()
            .position(|(model, _)| model.id == selected_model_id)
            .unwrap_or(0)
            + self.ai_filtered_reasoning_choices().len()
    }

    pub(in crate::features) fn ai_filtered_reasoning_choices(
        &self,
    ) -> Vec<nyaterm_core::AiReasoningEffort> {
        let models = self.ai_enabled_models();
        let selected_id = self.ai_selected_model_id();
        let model = models
            .iter()
            .find(|model| Some(&model.id) == selected_id.as_ref());
        let query = self.ai.discovery_query().trim().to_lowercase();
        nyaterm_core::ai::provider_settings::model_reasoning_options(model)
            .into_iter()
            .filter(|effort| {
                query.is_empty()
                    || format!(
                        "{} {effort:?}",
                        crate::features::ai::reasoning_effort_label(effort)
                    )
                    .to_lowercase()
                    .contains(&query)
            })
            .collect()
    }

    pub(in crate::features) fn select_ai_model_choice(&mut self, cx: &mut Context<Self>) {
        let reasoning = self.ai_filtered_reasoning_choices();
        let index = self.ai.discovery_index();
        if let Some(effort) = reasoning.get(index) {
            self.set_ai_reasoning_effort(effort.clone(), cx);
            return;
        }
        let choices = self.ai_filtered_model_choices();
        let Some((model, _)) = choices.get(index.saturating_sub(reasoning.len())).cloned() else {
            self.defer_ai_panel_snapshot_flush(cx);
            return;
        };
        self.ai.close_discovery_menu();
        self.set_ai_default_model(model.id.clone(), cx);
        self.ai
            .set_panel_status(format!("AI model selected: {}", model.name));
        self.defer_ai_panel_snapshot_flush(cx);
    }

    pub(in crate::features) fn handle_ai_model_search_key_down(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        self.mark_user_activity();
        let keystroke = &event.keystroke;
        if keystroke.modifiers.platform || keystroke.modifiers.alt || keystroke.modifiers.control {
            return false;
        }

        // The box owns the text; the menu owns the keys that walk and pick.
        let choice_count =
            self.ai_filtered_reasoning_choices().len() + self.ai_filtered_model_choices().len();
        match keystroke.key.as_str() {
            "escape" => {
                let selected_index = self.ai_selected_model_index();
                if self.ai.escape_discovery_search(selected_index) {
                    self.reset_text_input("ai.model-search", "", cx);
                }
                self.defer_ai_panel_snapshot_flush(cx);
            }
            "up" => {
                self.ai.move_discovery_index(choice_count, -1);
                self.defer_ai_panel_snapshot_flush(cx);
            }
            "down" => {
                self.ai.move_discovery_index(choice_count, 1);
                self.defer_ai_panel_snapshot_flush(cx);
            }
            "enter" => {
                if choice_count > 0 {
                    self.select_ai_model_choice(cx);
                } else {
                    self.defer_ai_panel_snapshot_flush(cx);
                }
            }
            _ => return false,
        }
        true
    }

    /// Apply an edit from the model search box.
    pub(in crate::features) fn apply_ai_model_search(
        &mut self,
        text: String,
        cx: &mut Context<Self>,
    ) {
        self.ai.set_discovery_query(text);
        self.defer_ai_panel_snapshot_flush(cx);
    }

    pub(in crate::features) fn ai_effective_target_session_ids(&self) -> Vec<String> {
        let mut session_ids = Vec::new();
        if let Some(active_session_id) = self.session.active_id()
            && self.session.session_info(active_session_id).is_some()
            && !self.session.is_disconnected(active_session_id)
        {
            session_ids.push(active_session_id.to_string());
        }
        for session_id in self.ai.chat_target_session_ids() {
            if !session_ids.iter().any(|id| id == session_id)
                && self.session.session_info(session_id).is_some()
                && !self.session.is_disconnected(session_id)
            {
                session_ids.push(session_id.clone());
            }
        }
        session_ids
    }

    pub(in crate::features) fn ai_current_owner_scope(&self) -> AiSessionScope {
        let Some(session_id) = self.session.active_id() else {
            return AiSessionScope::default();
        };
        let connection_ids = self
            .session
            .metadata(session_id)
            .and_then(|metadata| metadata.source_connection_id.clone())
            .into_iter()
            .collect();
        AiSessionScope {
            r#type: AiSessionScopeType::Terminal,
            target_id: Some(session_id.to_string()),
            connection_ids,
            label: self.session.display_name(session_id),
        }
    }

    pub(in crate::features) fn ai_terminal_context_for_sessions(
        &self,
        session_ids: &[String],
    ) -> AiContext {
        if session_ids.len() <= 1 {
            return self.ai_terminal_context_for_session(session_ids.first().map(String::as_str));
        }

        let per_session_line_limit =
            (self.ai.settings_context_line_limit() / session_ids.len()).max(1);
        let mut contexts = Vec::with_capacity(session_ids.len());
        for session_id in session_ids {
            contexts.push((
                self.session
                    .display_name(session_id)
                    .unwrap_or_else(|| compact_id(session_id)),
                self.ai_terminal_context_for_session_with_line_limit(
                    Some(session_id),
                    per_session_line_limit,
                ),
            ));
        }

        AiContext {
            connection_name: Some(
                contexts
                    .iter()
                    .map(|(_, context)| context.connection_name.as_deref().unwrap_or("-"))
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            host: Some(
                contexts
                    .iter()
                    .map(|(_, context)| context.host.as_deref().unwrap_or("-"))
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            port: contexts.first().and_then(|(_, context)| context.port),
            username: Some(
                contexts
                    .iter()
                    .map(|(_, context)| context.username.as_deref().unwrap_or("-"))
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            cwd: Some(
                contexts
                    .iter()
                    .map(|(_, context)| context.cwd.as_deref().unwrap_or("-"))
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            os: contexts.first().and_then(|(_, context)| context.os.clone()),
            arch: contexts
                .first()
                .and_then(|(_, context)| context.arch.clone()),
            recent_output: contexts
                .iter()
                .filter(|(_, context)| !context.recent_output.trim().is_empty())
                .map(|(label, context)| format!("[{label}]\n{}", context.recent_output))
                .collect::<Vec<_>>()
                .join("\n---\n"),
            selected_text: contexts
                .iter()
                .map(|(_, context)| context.selected_text.trim())
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
                .join("\n"),
            input_buffer: contexts
                .iter()
                .map(|(_, context)| context.input_buffer.trim())
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }

    pub(in crate::features) fn ai_terminal_targets_for_sessions(
        &self,
        session_ids: &[String],
    ) -> Vec<AiTerminalTarget> {
        session_ids
            .iter()
            .filter_map(|session_id| {
                let info = self.session.session_info(session_id)?;
                let context = self.ai_terminal_context_for_session(Some(session_id));
                Some(AiTerminalTarget {
                    terminal_session_id: session_id.clone(),
                    connection_id: self
                        .session
                        .metadata(session_id)
                        .and_then(|metadata| metadata.source_connection_id.clone()),
                    label: self
                        .session
                        .display_name(session_id)
                        .unwrap_or_else(|| compact_id(session_id)),
                    host: context.host,
                    username: context.username,
                    session_type: session_kind_label(info.kind).to_string(),
                })
            })
            .collect()
    }

    pub(in crate::features) fn ai_mention_candidates(&self) -> Vec<SessionInfo> {
        let query = self.ai.chat_mention_query().trim().to_ascii_lowercase();
        self.session
            .ordered_sessions()
            .into_iter()
            .filter(|session| !self.session.is_disconnected(&session.id))
            .filter(|session| {
                if query.is_empty() {
                    return true;
                }
                let display_name = self.session.display_name_by_info(session);
                display_name.to_ascii_lowercase().contains(&query)
                    || session.id.to_ascii_lowercase().contains(&query)
                    || session_kind_label(session.kind)
                        .to_ascii_lowercase()
                        .contains(&query)
            })
            .collect()
    }

    pub(in crate::features) fn remove_ai_target_session(
        &mut self,
        session_id: String,
        cx: &mut Context<Self>,
    ) {
        self.ai.remove_chat_target_session(&session_id);
        self.defer_ai_panel_snapshot_flush(cx);
    }

    pub(in crate::features) fn select_ai_mention_candidate(&mut self, cx: &mut Context<Self>) {
        let candidates = self.ai_mention_candidates();
        let Some(session) = candidates.get(self.ai.chat_mention_index()).cloned() else {
            self.ai.close_chat_mention();
            self.defer_ai_panel_snapshot_flush(cx);
            return;
        };
        let display_name = self.session.display_name_by_info(&session);
        self.ai.select_chat_mention(session.id, display_name);
        self.defer_ai_panel_snapshot_flush(cx);
    }

    pub(in crate::features) fn ai_terminal_context_for_session(
        &self,
        session_id: Option<&str>,
    ) -> AiContext {
        self.ai_terminal_context_for_session_with_line_limit(
            session_id,
            self.ai.settings_context_line_limit(),
        )
    }

    pub(in crate::features) fn ai_terminal_context_for_session_with_line_limit(
        &self,
        session_id: Option<&str>,
        line_limit: usize,
    ) -> AiContext {
        let metadata = session_id.and_then(|session_id| self.session.metadata(session_id));
        let ssh = match metadata.map(|metadata| &metadata.launch_config) {
            Some(SessionLaunchConfig::Ssh(config)) => Some(config.as_ref()),
            _ if session_id == self.session.active_id() => self.session.active_ssh_config(),
            _ => None,
        };
        let session = session_id.and_then(|session_id| self.session.session_info(session_id));
        let cwd = metadata
            .and_then(|metadata| match &metadata.launch_config {
                SessionLaunchConfig::Local(config) => config.working_dir.as_ref(),
                _ => None,
            })
            .or_else(|| {
                session
                    .as_ref()
                    .and_then(|session| session.working_dir.as_ref())
            });
        let recent_output = session_id
            .map(|session_id| self.terminal_buffer_text_for_session(session_id))
            .unwrap_or_else(|| self.active_terminal_buffer_text());
        let selected_text = if session_id.is_none() || session_id == self.session.active_id() {
            self.selected_terminal_text().unwrap_or_default()
        } else {
            String::new()
        };
        AiContext {
            connection_name: ssh
                .map(|config| config.name.clone())
                .or_else(|| session.as_ref().map(|session| session.name.clone())),
            host: ssh.map(|config| config.host.clone()),
            port: ssh.map(|config| config.port),
            username: ssh.map(|config| config.username.clone()),
            cwd: cwd.map(|path| path.display().to_string()),
            os: None,
            arch: Some(std::env::consts::ARCH.to_string()),
            recent_output: recent_terminal_output(&recent_output, line_limit.max(1)),
            selected_text,
            input_buffer: String::new(),
        }
    }

    pub(in crate::features) fn handle_ai_prompt_key_down(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        self.mark_user_activity();
        if self.ai.chat_or_agent_is_running() || !self.ai.settings_enabled() {
            self.ai.hide_chat_mention();
            self.defer_ai_panel_snapshot_flush(cx);
            return false;
        }
        let keystroke = &event.keystroke;
        if keystroke.modifiers.platform || keystroke.modifiers.alt || keystroke.modifiers.control {
            return false;
        }

        // While the @-mention list is open it owns the keys that walk and pick;
        // the box keeps the text either way.
        if self.ai.chat_mention_is_open() {
            let candidate_count = self.ai_mention_candidates().len();
            match keystroke.key.as_str() {
                "escape" => {
                    self.ai.close_chat_mention();
                    self.defer_ai_panel_snapshot_flush(cx);
                    return true;
                }
                "up" => {
                    self.ai.move_chat_mention_index(candidate_count, -1);
                    self.defer_ai_panel_snapshot_flush(cx);
                    return true;
                }
                "down" => {
                    self.ai.move_chat_mention_index(candidate_count, 1);
                    self.defer_ai_panel_snapshot_flush(cx);
                    return true;
                }
                _ => {}
            }
        }

        // The input's Submitted event handles Enter after IME composition commits.
        match keystroke.key.as_str() {
            "escape" => {
                self.ai.blur_chat_prompt();
                self.defer_ai_panel_snapshot_flush(cx);
            }
            _ => return false,
        }
        true
    }

    /// Put text into the prompt, from somewhere other than the box.
    ///
    /// The box owns its own buffer, so a caller that only wrote the draft would
    /// leave the two showing different things.
    pub(in crate::features) fn set_ai_prompt_draft(
        &mut self,
        text: impl Into<String>,
        cx: &mut Context<Self>,
    ) {
        let text = text.into();
        self.reset_text_input("ai.chat.prompt", &text, cx);
        self.ai.set_chat_prompt_draft(text);
        self.defer_ai_panel_snapshot_flush(cx);
    }

    /// Apply an edit from the AI prompt box.
    pub(in crate::features) fn apply_ai_prompt(&mut self, text: String, cx: &mut Context<Self>) {
        if !self.ai.settings_enabled() {
            return;
        }
        self.ai.set_chat_prompt_draft(text);
        self.defer_ai_panel_snapshot_flush(cx);
    }

    /// Deliver AI chat worker events as they arrive.
    ///
    /// Started once at window open. Before this the runtime tick polled
    /// `drain_chat_events`, so a streamed token batch waited for the next tick.
    pub(in crate::features) fn start_ai_chat_event_drain(&mut self, cx: &mut Context<Self>) {
        let Some(mut rx) = self.ai.take_chat_event_receiver() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            while let Some(event) = rx.next().await {
                if this
                    .update(cx, |this, cx| {
                        this.dispatch_ai_chat_event(event, cx);
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    pub(in crate::features) fn dispatch_ai_chat_event(
        &mut self,
        event: AiChatWorkerEvent,
        cx: &mut Context<Self>,
    ) {
        let stream_event = matches!(
            &event,
            AiChatWorkerEvent::Delta { .. } | AiChatWorkerEvent::AgentToolCallDelta { .. }
        );
        let before = self.ai_header_presentation();
        let Some(scope) = self.ai.scope_for_ai_session(event.session_id()) else {
            return;
        };
        let visible_scope = self.ai.active_scope_key().to_string();
        self.ai.switch_scope(&scope);
        let dirty = self.ai.chat_event_is_wanted() && self.apply_ai_chat_event(event, cx);
        self.ai.switch_scope(&visible_scope);
        if dirty && scope == visible_scope {
            if stream_event {
                self.schedule_ai_stream_refresh(cx);
            } else {
                self.flush_ai_panel_snapshot(cx);
            }
            self.notify_root_if_ai_header_changed(before, cx);
        }
    }

    pub(in crate::features) fn schedule_ai_stream_refresh(&mut self, cx: &mut Context<Self>) {
        let Some(generation) = self.ai.request_stream_refresh() else {
            return;
        };
        let timer = cx
            .background_executor()
            .timer(std::time::Duration::from_millis(33));
        cx.spawn(async move |this, cx| {
            timer.await;
            let _ = this.update(cx, |this, cx| {
                if this.ai.take_stream_refresh(generation) {
                    this.flush_ai_panel_snapshot(cx);
                }
            });
        })
        .detach();
    }

    /// Apply one worker event, reporting whether the UI needs a repaint.
    fn apply_ai_chat_event(&mut self, event: AiChatWorkerEvent, cx: &mut Context<Self>) -> bool {
        let mut dirty = false;
        match event {
            AiChatWorkerEvent::Delta {
                job_id,
                session_id,
                text_delta,
                reasoning_delta,
            } => {
                if self
                    .ai
                    .apply_chat_delta(job_id, &text_delta, reasoning_delta.as_deref())
                {
                    dirty = true;
                    self.settings
                        .update_store_status(format!("AI session {session_id} streaming"), true);
                }
            }
            AiChatWorkerEvent::AgentToolCallDelta {
                job_id,
                session_id,
                tool_name,
                arguments_delta_len,
            } => {
                if self
                    .ai
                    .apply_agent_tool_delta(job_id, tool_name.as_deref(), arguments_delta_len)
                {
                    dirty = true;
                    self.settings.update_store_status(
                        format!("AI session {session_id} streaming Agent tool call"),
                        true,
                    );
                }
            }
            AiChatWorkerEvent::AgentProtocolFallback { job_id, .. } => {
                dirty = self.ai.apply_agent_protocol_fallback(job_id);
            }
            AiChatWorkerEvent::AgentProtocolRepair { job_id, .. } => {
                dirty = self.ai.apply_agent_protocol_repair(job_id);
            }
            AiChatWorkerEvent::Finished(event) => {
                let native_decision = event
                    .result
                    .as_ref()
                    .ok()
                    .and_then(|output| output.native_decision.clone());
                if let Some(effect) =
                    self.ai
                        .finish_chat_job(event.job_id, event.session_id, event.result)
                {
                    dirty = true;
                    if let Some(call) = native_decision {
                        self.dispatch_native_agent_decision(call, cx);
                    } else if let Some(owner) = self.ai.ended_native_owner() {
                        self.clear_capability_owner(&owner, cx);
                    }
                    self.settings.update_store_status(
                        if effect.succeeded {
                            format!("AI session {} updated", effect.session_id)
                        } else {
                            self.ai.panel_status().to_string()
                        },
                        effect.succeeded,
                    );
                    if effect.clear_prompt_input
                        && self.ai.active_scope_key()
                            == self
                                .session
                                .active_id()
                                .map(|session_id| format!("terminal:{session_id}"))
                                .unwrap_or_else(|| "unbound:".to_string())
                    {
                        self.reset_text_input("ai.chat.prompt", "", cx);
                    }
                    if effect.refresh_usage_counts {
                        self.refresh_ai_usage_counts(cx);
                    }
                    if effect.auto_execute_first {
                        self.run_ai_command_card(0, cx);
                    }
                }
            }
        }
        let _ = cx;
        dirty
    }
}
