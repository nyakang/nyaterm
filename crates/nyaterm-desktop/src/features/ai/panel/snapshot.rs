use super::{
    AiAgentStepPresentation, AiHeaderPresentation, AiMentionCandidate, AiModelChoice,
    AiPanelChrome, AiPanelSnapshot, AiTargetSession, harness,
};
use crate::features::NyaTermApp;
use crate::features::formatting::short_id;
use crate::features::text_inputs::TextInputSetup;
use gpui::Context;
use gpui::RenderImage;
use nyaterm_core::AiAction;
use nyaterm_core::AiAgentKind;
use nyaterm_core::AiMode;
use nyaterm_core::AiModelConfigItem;
use nyaterm_core::AiProviderKind;
use nyaterm_core::AiReasoningEffort;
use nyaterm_core::truncate_preview;
use rust_i18n::t;
use std::sync::Arc;

impl NyaTermApp {
    pub(in crate::features) fn ai_header_presentation(&self) -> AiHeaderPresentation {
        let selected_model_id = self.ai_selected_model_id();
        let mut model_label = selected_model_id
            .as_deref()
            .and_then(|model_id| {
                self.ai
                    .settings_config()
                    .models
                    .iter()
                    .find(|model| model.id == model_id)
                    .map(|model| truncate_preview(&model.name, 28))
            })
            .unwrap_or_else(|| t!("ai.notConfigured").to_string());
        if self.ai.chat_run_mode() == AiMode::Agent {
            match self.ai.chat_agent_kind() {
                AiAgentKind::Codex => {
                    model_label = self
                        .ai
                        .settings_config()
                        .codex
                        .default_model
                        .clone()
                        .filter(|model| !model.trim().is_empty())
                        .unwrap_or_else(|| "Codex".to_string())
                }
                AiAgentKind::ClaudeCode => {
                    model_label = self
                        .ai
                        .settings_config()
                        .claude_code
                        .default_model
                        .clone()
                        .filter(|model| !model.trim().is_empty())
                        .unwrap_or_else(|| "Claude Code".to_string())
                }
                AiAgentKind::Nyaterm => {}
            }
        }
        if let Some(active_id) = self.session.active_id() {
            let label = self
                .session
                .display_name(active_id)
                .unwrap_or_else(|| short_id(active_id).to_string());
            let targets = self.ai_effective_target_session_ids();
            let count = targets.iter().filter(|id| id.as_str() != active_id).count();
            model_label = if count > 0 {
                t!("ai.panelMetaMultiTarget", target = label, count = count).to_string()
            } else {
                label
            };
        }
        AiHeaderPresentation {
            running: self.ai.chat_or_agent_is_running(),
            selected_model_id,
            model_label,
            execution_mode: self
                .ai
                .settings_config()
                .agent_command_execution_mode
                .clone(),
        }
    }

    pub(in crate::features) fn notify_root_if_ai_header_changed(
        &self,
        before: AiHeaderPresentation,
        cx: &mut Context<Self>,
    ) -> bool {
        if before == self.ai_header_presentation() {
            return false;
        }
        cx.notify();
        true
    }

    pub(in crate::features) fn defer_ai_panel_snapshot_flush(&mut self, cx: &mut Context<Self>) {
        if !self.ai.request_panel_refresh() {
            return;
        }
        self.defer_app_update(cx, |app, cx| {
            if !app.ai.take_panel_refresh_request() {
                return;
            }
            app.flush_ai_panel_snapshot(cx);
        });
    }

    pub(in crate::features) fn flush_ai_panel_snapshot(&mut self, cx: &mut Context<Self>) {
        self.ai.clear_panel_refresh_request();
        let snapshot = self.build_ai_panel_snapshot(cx);
        let panel = self.ai_panel.clone();
        panel.update(cx, |panel, cx| panel.set_snapshot(snapshot, cx));
    }

    fn ai_model_provider_presentation(
        &self,
        model: &AiModelConfigItem,
    ) -> (Option<AiProviderKind>, Option<Arc<RenderImage>>) {
        let credential = model.credential_id.as_ref().and_then(|id| {
            self.ai
                .settings_config()
                .provider_credentials
                .iter()
                .find(|credential| &credential.id == id)
        });
        (
            credential
                .map(|credential| credential.provider_kind.clone())
                .or_else(|| model.provider_kind.clone()),
            model
                .credential_id
                .as_ref()
                .and_then(|id| self.ai.provider_view().icons.get(id).cloned()),
        )
    }

    fn build_ai_panel_snapshot(&mut self, cx: &mut Context<Self>) -> AiPanelSnapshot {
        let palette = self.theme_palette();
        let enabled = self.ai.settings_config().enabled;
        let agent_mode = self.ai.chat_run_mode() == AiMode::Agent;
        let running = self.ai.chat_or_agent_is_running();
        let agent_kind = self.ai.chat_agent_kind();
        let external_agent = agent_mode && agent_kind != AiAgentKind::Nyaterm;
        let selected_model_id = self.ai_selected_model_id();
        let enabled_models: Arc<[AiModelConfigItem]> = self.ai_enabled_models().into();
        let selected_model_exists = selected_model_id
            .as_deref()
            .is_some_and(|model_id| enabled_models.iter().any(|model| model.id == model_id));
        let model_label = selected_model_id
            .as_deref()
            .and_then(|model_id| enabled_models.iter().find(|model| model.id == model_id))
            .map(|model| model.name.clone())
            .unwrap_or_else(|| t!("ai.notConfigured").to_string());
        let selected_model = enabled_models
            .iter()
            .find(|model| Some(&model.id) == selected_model_id.as_ref());
        let (selected_provider_kind, selected_provider_icon) = selected_model
            .map(|model| self.ai_model_provider_presentation(model))
            .unwrap_or_default();
        let reasoning_choices: Arc<[AiReasoningEffort]> =
            self.ai_filtered_reasoning_choices().into();
        let model_choices_vec = self.ai_filtered_model_choices();
        self.ai
            .clamp_discovery_index(reasoning_choices.len() + model_choices_vec.len());
        let model_choices = model_choices_vec
            .into_iter()
            .map(|(model, provider_label)| {
                let (provider_kind, provider_icon) = self.ai_model_provider_presentation(&model);
                AiModelChoice {
                    model,
                    provider_label,
                    provider_kind,
                    provider_icon,
                }
            })
            .collect::<Vec<_>>()
            .into();
        let target_session_ids = self.ai.chat_target_session_ids().to_vec();
        let target_sessions = target_session_ids
            .iter()
            .filter_map(|session_id| {
                self.session
                    .session_info(session_id)
                    .map(|session| AiTargetSession {
                        session_id: session_id.clone(),
                        label: self.session.display_name_by_info(&session),
                    })
            })
            .collect::<Vec<_>>()
            .into();
        let mention_candidates: Arc<[AiMentionCandidate]> = if self.ai.chat_mention_is_open() {
            self.ai_mention_candidates()
                .into_iter()
                .map(|session| AiMentionCandidate {
                    selected: target_session_ids
                        .iter()
                        .any(|session_id| session_id == &session.id),
                    kind: crate::features::formatting::session_kind_label(session.kind).to_string(),
                    label: self.session.display_name_by_info(&session),
                    session_id: session.id,
                })
                .collect::<Vec<_>>()
                .into()
        } else {
            Vec::<AiMentionCandidate>::new().into()
        };
        self.ai.clamp_chat_mention_index(mention_candidates.len());

        let prompt_placeholder = if !enabled {
            t!("ai.goToSettingsToEnable")
        } else {
            t!("ai.placeholder")
        };
        let prompt_draft = self.ai.chat_prompt_draft().to_string();
        self.ensure_text_input(
            "ai.chat.prompt",
            &prompt_draft,
            TextInputSetup::multi_line(prompt_placeholder.clone()).submit_on_enter(),
            cx,
        );
        let prompt_input = self
            .existing_text_input("ai.chat.prompt")
            .expect("AI prompt input was just built");

        prompt_input.update(cx, |input, cx| {
            input.set_disabled(running || !enabled, cx);
            input.set_placeholder(prompt_placeholder, cx);
        });

        let model_search_input = if self.ai.discovery_menu_is_open() {
            let query = self.ai.discovery_query().to_string();
            self.ensure_text_input(
                "ai.model-search",
                &query,
                TextInputSetup::placeholder(t!("ai.searchModels")),
                cx,
            );
            self.existing_text_input("ai.model-search")
        } else {
            None
        };
        let history_search_input = if self.ai.history_is_open() {
            let query = self.ai.history_query().to_string();
            self.ensure_text_input(
                "ai.history-search",
                &query,
                TextInputSetup::placeholder(t!("ai.historySearchPlaceholder")),
                cx,
            );
            self.existing_text_input("ai.history-search")
        } else {
            None
        };
        let native_run = self.ai.native_run_view();
        let native_approvals = native_run
            .as_ref()
            .filter(|view| {
                self.ai
                    .native_run_is_live(&view.run_id, self.ai.chat_session_id())
            })
            .map(|view| {
                let mut approvals = self
                    .mcp
                    .capabilities
                    .native_approval_requests(&view.run_id, self.ai.chat_session_id());
                for view in &mut approvals {
                    if let Some(target) = &view.approval.target
                        && let Some(label) = self.session.display_name(target)
                    {
                        view.approval.target = Some(format!("{label} ({target})"));
                    }
                }
                approvals
            })
            .unwrap_or_default()
            .into();
        let mut native_answer_inputs = Vec::new();
        if let Some(view) = &native_run
            && view.status == nyaterm_core::ai::harness::AgentRunStatus::WaitingForUser
            && let Some(call_id) = &view.call_id
        {
            for (index, question) in view.questions.iter().enumerate() {
                let id = harness::answer_input_id(&view.run_id, call_id, index);
                self.ensure_text_input(
                    id.clone(),
                    view.answers
                        .get(&question.id)
                        .map(String::as_str)
                        .unwrap_or_default(),
                    TextInputSetup::placeholder(t!("ai.harness.answerPlaceholder"))
                        .submit_on_enter(),
                    cx,
                );
                if let Some(input) = self.existing_text_input(&id) {
                    native_answer_inputs.push(input);
                }
            }
        }
        let (viewport_width, viewport_height) = self.shell.viewport_size();

        AiPanelSnapshot {
            index: Arc::default(),
            native_run,
            native_approvals,
            native_answer_inputs,
            ui_font_family: if self.settings.summary().ui_font_family.trim().is_empty() {
                crate::features::shell::gpui_ui_font_fallback().into()
            } else {
                self.gpui_ui_font().family.into()
            },
            chrome: AiPanelChrome {
                palette,
                transparent_surface: self.shell_transparent_color(palette.surface),
                transparent_section_header: self.shell_transparent_color(palette.section_header),
                surface: self.shell_surface_color(palette.surface),
                viewport_width,
                viewport_height,
            },
            enabled,
            agent_mode,
            running,
            agent_kind,
            reasoning_effort: self.ai.settings_config().default_reasoning_effort.clone(),
            reasoning_choices,
            codex_enabled: self.ai.settings_config().codex.enabled,
            claude_code_enabled: self.ai.settings_config().claude_code.enabled,
            external_agent,
            external_model_label: match self.ai.chat_agent_kind() {
                AiAgentKind::Codex => self
                    .ai
                    .settings_config()
                    .codex
                    .default_model
                    .clone()
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| "Codex".to_string()),
                AiAgentKind::ClaudeCode => self
                    .ai
                    .settings_config()
                    .claude_code
                    .default_model
                    .clone()
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| "Claude Code".to_string()),
                AiAgentKind::Nyaterm => String::new(),
            },
            selected_model_id,
            selected_model_exists,
            model_label,
            selected_provider_kind,
            selected_provider_icon,
            enabled_models,
            model_choices,
            discovery_menu_open: self.ai.discovery_menu_is_open(),
            discovery_index: self.ai.discovery_index(),
            prompt_draft,
            prompt_input,
            model_search_input,
            history_search_input,
            file_action_ready: self
                .ai
                .chat_prepared_request()
                .is_some_and(|request| request.action == AiAction::CustomFileAction),
            messages: self.ai.chat_snapshot_messages(),
            streaming_assistant_id: self.ai.chat_streaming_assistant_id().map(str::to_string),
            response_phase: self.ai.response_phase(),
            expanded_message_thoughts: self
                .ai
                .expanded_message_thoughts()
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .into(),
            expanded_command_details: self
                .ai
                .expanded_command_details()
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .into(),
            expanded_command_scripts: self
                .ai
                .expanded_command_scripts()
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .into(),
            agent_history_expanded: self.ai.agent_history_expanded(),
            expanded_execution_groups: self
                .ai
                .expanded_execution_groups()
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .into(),
            command_cards: self.ai.chat_command_cards().to_vec().into(),
            agent_steps: self
                .ai
                .agent_steps()
                .iter()
                .cloned()
                .map(|step| AiAgentStepPresentation {
                    thought_open: self.ai.agent_thought_is_expanded(step.step_index),
                    output_open: self.ai.agent_output_is_expanded(step.step_index),
                    step,
                })
                .collect::<Vec<_>>()
                .into(),
            target_sessions,
            mention_open: self.ai.chat_mention_is_open(),
            mention_index: self.ai.chat_mention_index(),
            mention_candidates,
            quoted_text: self.ai.chat_quote().map(str::to_string),
            detected_error: self.ai.panel_detected_error().cloned(),
            message_menu: self.ai.chat_message_menu().cloned(),
            history_open: self.ai.history_is_open(),
            history_query: self.ai.history_query().to_string(),
            history_sessions: self.ai.history_sessions().to_vec().into(),
            history_running_ids: self
                .ai
                .history_sessions()
                .iter()
                .filter(|session| self.ai.ai_session_is_running(&session.id))
                .map(|session| session.id.clone())
                .collect::<Vec<_>>()
                .into(),
            current_ai_session_id: self.ai.chat_session_id().to_string(),
            owner_terminal_id: self.session.active_id().map(str::to_string),
            owner_connection_id: self
                .session
                .active_id()
                .and_then(|id| self.session.metadata(id))
                .and_then(|metadata| metadata.source_connection_id.clone()),
            history_pending: self.ai.history_is_pending(),
            history_error: self.ai.history_error().map(str::to_string),
            history_actions_disabled: self.ai.history_actions_are_disabled(),
            execution_menu_open: self.ai.panel_execution_menu_is_open(),
            command_execution_mode: self
                .ai
                .settings_config()
                .agent_command_execution_mode
                .clone(),
            background_execution_enabled: self
                .ai
                .settings_config()
                .agent_background_execution_enabled,
        }
    }

    pub(in crate::features) fn sync_ai_active_scope(&mut self, cx: &mut Context<Self>) {
        let scope = self
            .session
            .active_id()
            .map(|session_id| format!("terminal:{session_id}"))
            .unwrap_or_else(|| "unbound:".to_string());
        if self.ai.switch_visible_scope(&scope) {
            let draft = self.ai.chat_prompt_draft().to_string();
            self.reset_text_input("ai.chat.prompt", &draft, cx);
        }
    }
}
