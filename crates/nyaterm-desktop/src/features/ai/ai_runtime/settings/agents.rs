use futures::StreamExt as _;
use gpui::Context;

use crate::features::NyaTermApp;
use crate::features::ai::agent_management::AgentCommand;

impl NyaTermApp {
    pub(in crate::features) fn edit_acp_profile(
        &mut self,
        id: &str,
        action: &str,
        value: String,
        cx: &mut Context<Self>,
    ) {
        if matches!(action, "advanced" | "request-delete" | "cancel-delete") {
            self.ai.agent_editor_action(id, action);
            self.request_settings_panel_refresh(cx);
            return;
        }
        match self.ai.edit_external_profile(id, action, value) {
            Ok(true) => {
                self.ensure_ai_agent_inputs(cx);
                self.defer_settings_persistence(cx);
                self.defer_ai_panel_snapshot_flush(cx);
            }
            Err(error) => self.ai.set_agent_management_error(error.into()),
            _ => {}
        }
        self.request_settings_panel_refresh(cx);
    }

    pub(in crate::features) fn apply_acp_profile_input(
        &mut self,
        rest: &str,
        text: String,
        cx: &mut Context<Self>,
    ) {
        if let Some((id, field)) = rest.split_once('.') {
            self.edit_acp_profile(id, field, text, cx);
        }
    }

    pub(in crate::features) fn inspect_acp_profile(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.ai.agent_editor_view().errors.contains_key(id) {
            return;
        }
        if let Some(profile) =
            nyaterm_core::ai::external_agents::profiles(&self.ai.pending_settings())
                .iter()
                .find(|p| p.id == id)
                .cloned()
        {
            self.submit_ai_agent_command(
                AgentCommand::Acp {
                    profile: Box::new(profile),
                    operation: nyaterm_agent_client::acp::AgentOperation::Inspect,
                },
                cx,
            );
        }
    }

    pub(in crate::features) fn refresh_ai_agents(&mut self, cx: &mut Context<Self>) {
        self.submit_ai_agent_command(
            AgentCommand::Refresh(
                nyaterm_core::ai::external_agents::profiles(&self.ai.pending_settings())
                    .iter()
                    .filter(|profile| !self.ai.agent_editor_view().errors.contains_key(&profile.id))
                    .cloned()
                    .collect(),
            ),
            cx,
        );
    }

    pub(in crate::features) fn acp_account_action(
        &mut self,
        id: &str,
        method: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if let Some(profile) =
            nyaterm_core::ai::external_agents::profiles(&self.ai.pending_settings())
                .iter()
                .find(|p| p.id == id)
                .cloned()
        {
            let operation = match method {
                Some(method) => nyaterm_agent_client::acp::AgentOperation::Authenticate { method },
                None => nyaterm_agent_client::acp::AgentOperation::Logout,
            };
            self.submit_ai_agent_command(
                AgentCommand::Acp {
                    profile: Box::new(profile),
                    operation,
                },
                cx,
            );
        }
    }

    pub(in crate::features) fn copy_external_mcp_config(
        &mut self,
        client: &str,
        cx: &mut Context<Self>,
    ) {
        let result = if self.http_mcp_settings().transport
            == nyaterm_core::ai::mcp_http::McpTransport::StreamableHttp
        {
            self.desktop_controller
                .as_ref()
                .and_then(|controller| controller.upgrade())
                .ok_or_else(|| "HTTP MCP is unavailable".to_owned())
                .and_then(|controller| controller.read(cx).http_mcp_config(client))
        } else if matches!(
            self.http_mcp_settings().transport,
            nyaterm_core::ai::mcp_http::McpTransport::Unknown(_)
        ) {
            Err("Unsupported MCP transport".to_owned())
        } else {
            self.ai
                .agent_management_view()
                .mcp_helper_path
                .as_ref()
                .ok_or_else(|| {
                    "NyaTerm MCP helper is unavailable; refresh agent status".to_string()
                })
                .and_then(|path| {
                    nyaterm_core::ai::mcp_client::client_config(
                        &nyaterm_core::ai::mcp_client::McpClient::from(client.to_owned()),
                        nyaterm_core::ai::mcp_client::ConfigTransport::Stdio {
                            command: &path.to_string_lossy(),
                        },
                    )
                    .map_err(str::to_owned)
                })
        };
        match result {
            Ok(config) => {
                self.copy_text_with_feedback("copy-mcp-config", config, cx);
            }
            Err(error) => {
                self.ai.set_agent_management_error(error);
                self.notify_operation(
                    "copy-mcp-config",
                    nyaterm_ui::notification::NyaNotificationKind::Error,
                    rust_i18n::t!("common.copyFailed").to_string(),
                    cx,
                );
            }
        }
        self.request_settings_panel_refresh(cx);
    }

    pub(in crate::features) fn set_mcp_config_client(
        &mut self,
        value: &str,
        cx: &mut Context<Self>,
    ) {
        let client = nyaterm_core::ai::mcp_client::McpClient::from(value.to_owned());
        if !client.supports(&self.ai.settings_config().external_mcp.transport) {
            return;
        }
        self.ai.set_settings_mcp_client(client);
        self.persist_ai_settings_now(cx);
        self.request_settings_panel_refresh(cx);
    }

    fn submit_ai_agent_command(&mut self, command: AgentCommand, cx: &mut Context<Self>) {
        if self.ai.agent_management_view().pending {
            return;
        }
        let store = self.store_blocking_client();
        let started =
            self.ai
                .submit_agent_command(command, store, cx.background_executor().clone());
        self.request_settings_panel_refresh(cx);
        if !started {
            return;
        }
        let Some(mut events) = self.ai.take_agent_events() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            while let Some(event) = events.next().await {
                if this
                    .update(cx, |this, cx| {
                        let auth_url = this.ai.apply_agent_event(event);
                        if let Some(url) = auth_url
                            && let Ok(url) = url::Url::parse(&url)
                            && matches!(url.scheme(), "https" | "http")
                        {
                            cx.open_url(url.as_str());
                        }
                        this.request_settings_panel_refresh(cx);
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }
}
