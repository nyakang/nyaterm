//! Desktop adapters for the process-owned HTTP service.
use crate::features::NyaTermApp;
use gpui::Context;
use nyaterm_core::ai::mcp_http::McpTransport;
use nyaterm_core::{CapabilityScope, ExternalMcpSessionScope, ExternalMcpSettings};
use nyaterm_mcp_host::server::{McpHostEvent, McpHostRuntime};

impl NyaTermApp {
    pub(crate) fn http_mcp_helper() -> Result<std::path::PathBuf, String> {
        crate::features::ai::resolve_mcp_helper()
    }
    pub(in crate::features) fn http_mcp_view(
        &self,
        cx: &gpui::App,
    ) -> crate::app_shell::HttpMcpView {
        self.desktop_controller
            .as_ref()
            .and_then(|controller| controller.upgrade())
            .map(|controller| {
                let mut view = controller.read(cx).http_mcp_view();
                view.owned_by_current_window = view.owner == Some(self.workspace_id);
                view
            })
            .unwrap_or_default()
    }

    pub(crate) fn http_mcp_settings(&self) -> ExternalMcpSettings {
        self.ai.settings_config().external_mcp.clone()
    }
    pub(crate) fn http_mcp_bridge_parameters(
        &self,
        settings: &ExternalMcpSettings,
    ) -> (
        CapabilityScope,
        futures::channel::mpsc::UnboundedSender<McpHostEvent>,
    ) {
        let scope = match settings.session_scope {
            ExternalMcpSessionScope::CurrentWindow => CapabilityScope::CurrentWindow {
                owner_window_label: self.mcp.owner_window_label.clone(),
            },
            ExternalMcpSessionScope::AllSessions => CapabilityScope::AllSessions,
        };
        (scope, self.mcp.request_sender.clone())
    }
    pub(crate) fn install_http_mcp_bridge(
        &mut self,
        runtime: McpHostRuntime,
        cx: &mut Context<Self>,
    ) {
        self.retire_http_mcp_bridge(None, cx);
        self.mcp.generation = Some(runtime.endpoint().generation.clone());
        self.mcp.runtime = Some(runtime);
        self.mcp.public_http = true;
    }
    pub(crate) fn retire_http_mcp_bridge(
        &mut self,
        generation: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        if generation.is_some_and(|value| self.mcp.generation.as_deref() != Some(value)) {
            return;
        }
        if let Some(generation) = self.mcp.generation.take() {
            self.mcp.capabilities.revoke_generation(&generation);
        }
        self.mcp.public_http = false;
        if let Some(runtime) = self.mcp.runtime.take() {
            cx.background_executor()
                .spawn(async move {
                    drop(runtime);
                })
                .detach();
        }
        self.request_settings_panel_refresh(cx);
    }
    pub(crate) fn refresh_http_mcp_settings(&mut self, cx: &mut Context<Self>) {
        self.request_settings_panel_refresh(cx);
        cx.notify();
    }
    pub(in crate::features) fn reconcile_mcp_after_shared_settings(
        &mut self,
        previous: &ExternalMcpSettings,
        cx: &mut Context<Self>,
    ) {
        let current = self.http_mcp_settings();
        if current.runtime_eq(previous) {
            return;
        }
        match current.transport {
            McpTransport::Stdio => {
                self.retire_http_mcp_bridge(None, cx);
                let _ = self.reconfigure_mcp_host();
            }
            McpTransport::StreamableHttp => {
                if !self.mcp.public_http {
                    self.retire_http_mcp_bridge(None, cx);
                }
            }
            McpTransport::Unknown(_) => self.retire_http_mcp_bridge(None, cx),
        }
        self.schedule_http_mcp(false, cx);
    }
    pub(in crate::features) fn schedule_http_mcp(&self, claim: bool, cx: &mut Context<Self>) {
        let Some(controller) = self.desktop_controller.clone() else {
            return;
        };
        let app = cx.entity().downgrade();
        let workspace = self.workspace_id;
        let settings = self.http_mcp_settings();
        cx.defer(move |cx| {
            let _ = controller.update(cx, |controller, cx| {
                controller.configure_http_mcp(workspace, app, settings, claim, cx)
            });
        });
    }
    pub(in crate::features) fn set_ai_mcp_transport(
        &mut self,
        value: &str,
        cx: &mut Context<Self>,
    ) {
        let transport = McpTransport::from(value.to_owned());
        if matches!(transport, McpTransport::Unknown(_)) {
            return;
        }
        self.ai.set_settings_mcp_transport(transport);
        self.persist_ai_settings_now(cx);
        if self.http_mcp_settings().transport != McpTransport::Stdio {
            self.retire_http_mcp_bridge(None, cx);
        } else {
            let _ = self.reconfigure_mcp_host();
        }
        self.schedule_http_mcp(true, cx);
        self.request_settings_panel_refresh(cx);
    }
    pub(in crate::features) fn apply_http_mcp_form(&mut self, cx: &mut Context<Self>) {
        let port = self
            .existing_text_input("ai.mcp.http-port")
            .map(|input| input.read(cx).value(cx))
            .unwrap_or_default();
        let origins = self
            .existing_text_input("ai.mcp.http-origins")
            .map(|input| input.read(cx).value(cx))
            .unwrap_or_default();
        let origins: Vec<String> = origins
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect();
        let result = port
            .parse::<u16>()
            .map_err(|_| "HTTP port must be between 1 and 65535")
            .and_then(|port| {
                nyaterm_core::ai::mcp_http::validate_http_settings(port, &origins)
                    .map(|origins| (port, origins))
            });
        match result {
            Ok((port, origins)) => {
                self.ai.set_settings_mcp_http(port, origins);
                self.persist_ai_settings_now(cx);
                self.schedule_http_mcp(false, cx);
            }
            Err(error) => self.ai.set_agent_management_error(error.to_owned()),
        }
        self.request_settings_panel_refresh(cx);
    }
}

#[cfg(test)]
mod tests {
    use crate::features::test_support::app_with_visible_local_session;
    use crate::test_support::{TestAppCleanup, TestConfigDir};
    use gpui::TestAppContext;
    use nyaterm_core::ai::mcp_client::McpClient;

    #[test]
    fn client_preference_sync_preserves_the_stdio_host_generation() {
        let root = TestConfigDir::new("nyaterm-mcp-client-preference-sync");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, root.path(), "local");
        app.update(&mut cx, |app, cx| {
            app.ai.settings_config_mut().external_mcp.enabled = true;
            let generation = app.mcp.enable_public_for_test();
            let previous = app.http_mcp_settings();
            app.ai.set_settings_mcp_client(McpClient::VsCode);
            app.reconcile_mcp_after_shared_settings(&previous, cx);
            assert!(app.mcp.generation_matches(&generation));
            assert_eq!(
                app.http_mcp_settings().selected_client(),
                &McpClient::VsCode
            );
        });
        let cleanup = app.update(&mut cx, |app, cx| app.shutdown_blocking_jobs(cx));
        cx.foreground_executor().block_test(cleanup);
    }
}
