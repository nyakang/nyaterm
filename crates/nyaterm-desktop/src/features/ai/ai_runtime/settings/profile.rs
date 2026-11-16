use gpui::Context;
use nyaterm_core::ai::mcp_execution::McpTerminalExecutionMode;
use nyaterm_core::{
    AgentCommandExecutionMode, AiAgentKind, AiMode, AiPermissionMode, AiReasoningEffort,
    CodexThreadMode, ExternalMcpSessionScope, McpTerminalDisplayMode, RiskLevel,
};

use crate::features::NyaTermApp;
use crate::features::ai::AiFullAccessSetting;

impl NyaTermApp {
    pub(in crate::features) fn toggle_ai_enabled(&mut self, cx: &mut Context<Self>) {
        self.ai.toggle_settings_enabled();
        self.persist_ai_settings_now(cx);
    }

    pub(in crate::features) fn set_ai_run_mode(
        &mut self,
        mode: AiMode,
        kind: AiAgentKind,
        cx: &mut Context<Self>,
    ) {
        let before = self.ai_header_presentation();
        if self.ai.set_chat_run_mode(mode, kind) {
            self.persist_ai_settings_now(cx);
            self.notify_root_if_ai_header_changed(before, cx);
        }
    }

    pub(in crate::features) fn set_ai_reasoning_effort(
        &mut self,
        effort: AiReasoningEffort,
        cx: &mut Context<Self>,
    ) {
        self.ai.set_settings_reasoning_effort(effort);
        self.persist_ai_settings_now(cx);
    }

    pub(in crate::features) fn set_ai_command_mode(
        &mut self,
        mode: AgentCommandExecutionMode,
        cx: &mut Context<Self>,
    ) {
        let before = self.ai_header_presentation();
        self.ai.set_settings_command_mode(mode);
        self.persist_ai_settings_now(cx);
        self.notify_root_if_ai_header_changed(before, cx);
    }

    pub(in crate::features) fn toggle_ai_background_execution(&mut self, cx: &mut Context<Self>) {
        self.ai.toggle_settings_background_execution();
        self.persist_ai_settings_now(cx);
    }

    pub(in crate::features) fn toggle_ai_redaction(&mut self, cx: &mut Context<Self>) {
        self.ai.toggle_settings_redaction();
        self.persist_ai_settings_now(cx);
    }

    pub(in crate::features) fn toggle_ai_allow_save_command(&mut self, cx: &mut Context<Self>) {
        self.ai.toggle_settings_allow_save_command();
        self.persist_ai_settings_now(cx);
    }

    pub(in crate::features) fn toggle_ai_record_history(&mut self, cx: &mut Context<Self>) {
        self.ai.toggle_settings_record_history();
        self.persist_ai_settings_now(cx);
    }

    pub(in crate::features) fn set_ai_context_line_limit(
        &mut self,
        value: u32,
        cx: &mut Context<Self>,
    ) {
        self.ai.set_settings_context_line_limit(value);
        self.persist_ai_settings_now(cx);
    }

    pub(in crate::features) fn set_ai_timeout_ms(&mut self, value: u64, cx: &mut Context<Self>) {
        self.ai.set_settings_timeout_ms(value);
        self.persist_ai_settings_now(cx);
    }

    pub(in crate::features) fn set_ai_agent_steps(&mut self, value: u16, cx: &mut Context<Self>) {
        self.ai.set_settings_agent_steps(value);
        self.persist_ai_settings_now(cx);
    }

    pub(in crate::features) fn set_ai_agent_step_timeout_ms(
        &mut self,
        value: u64,
        cx: &mut Context<Self>,
    ) {
        self.ai.set_settings_agent_step_timeout_ms(value);
        self.persist_ai_settings_now(cx);
    }

    pub(in crate::features) fn set_ai_terminal_output_lines(
        &mut self,
        value: u16,
        cx: &mut Context<Self>,
    ) {
        self.ai.set_settings_terminal_output_lines(value);
        self.persist_ai_settings_now(cx);
    }

    pub(in crate::features) fn set_ai_file_size_mb(&mut self, value: u64, cx: &mut Context<Self>) {
        self.ai.set_settings_file_size_mb(value);
        self.persist_ai_settings_now(cx);
    }

    pub(in crate::features) fn update_ai_smart_auto_execute_max_risk(
        &mut self,
        risk: RiskLevel,
        cx: &mut Context<Self>,
    ) {
        self.ai.set_settings_smart_auto_execute_max_risk(risk);
        self.persist_ai_settings_now(cx);
    }

    pub(in crate::features) fn set_ai_default_agent(
        &mut self,
        kind: AiAgentKind,
        cx: &mut Context<Self>,
    ) {
        self.ai.set_settings_default_agent(kind);
        self.persist_ai_settings_now(cx);
    }

    pub(in crate::features) fn request_ai_permission_mode(
        &mut self,
        setting: AiFullAccessSetting,
        mode: AiPermissionMode,
        cx: &mut Context<Self>,
    ) {
        if self.ai.request_settings_permission_mode(setting, mode) {
            self.persist_ai_settings_now(cx);
            if setting == AiFullAccessSetting::McpHost {
                self.reconfigure_mcp_host_and_report();
            }
        }
        cx.notify();
    }

    pub(in crate::features) fn pending_ai_full_access(&self) -> Option<AiFullAccessSetting> {
        self.ai.pending_full_access_setting()
    }

    pub(in crate::features) fn cancel_ai_full_access(&mut self, cx: &mut Context<Self>) {
        self.ai.cancel_settings_full_access();
        cx.notify();
    }

    pub(in crate::features) fn confirm_ai_full_access(&mut self, cx: &mut Context<Self>) {
        let Some(setting) = self.ai.confirm_settings_full_access() else {
            return;
        };
        self.persist_ai_settings_now(cx);
        if setting == AiFullAccessSetting::McpHost {
            self.reconfigure_mcp_host_and_report();
        }
        cx.notify();
    }

    pub(in crate::features) fn toggle_ai_codex_enabled(&mut self, cx: &mut Context<Self>) {
        self.ai.toggle_settings_codex_enabled();
        self.persist_ai_settings_now(cx);
    }

    pub(in crate::features) fn toggle_ai_codex_mcp_integration(&mut self, cx: &mut Context<Self>) {
        self.ai.toggle_settings_codex_mcp_integration();
        self.persist_ai_settings_now(cx);
    }

    pub(in crate::features) fn toggle_ai_claude_enabled(&mut self, cx: &mut Context<Self>) {
        self.ai.toggle_settings_claude_enabled();
        self.persist_ai_settings_now(cx);
    }

    pub(in crate::features) fn toggle_ai_claude_mcp_integration(&mut self, cx: &mut Context<Self>) {
        self.ai.toggle_settings_claude_mcp_integration();
        self.persist_ai_settings_now(cx);
    }

    pub(in crate::features) fn toggle_ai_mcp_enabled(&mut self, cx: &mut Context<Self>) {
        self.ai.toggle_settings_mcp_enabled();
        self.persist_ai_settings_now(cx);
        self.reconfigure_mcp_host_and_report();
        cx.notify();
    }

    pub(in crate::features) fn set_ai_codex_thread_mode(
        &mut self,
        mode: CodexThreadMode,
        cx: &mut Context<Self>,
    ) {
        self.ai.set_settings_codex_thread_mode(mode);
        self.persist_ai_settings_now(cx);
    }

    pub(in crate::features) fn set_ai_mcp_scope(
        &mut self,
        scope: ExternalMcpSessionScope,
        cx: &mut Context<Self>,
    ) {
        self.ai.set_settings_mcp_scope(scope);
        self.persist_ai_settings_now(cx);
        self.reconfigure_mcp_host_and_report();
        cx.notify();
    }

    pub(in crate::features) fn set_ai_mcp_terminal_display_mode(
        &mut self,
        mode: McpTerminalDisplayMode,
        cx: &mut Context<Self>,
    ) {
        self.ai.set_settings_mcp_terminal_display_mode(mode);
        self.persist_ai_settings_now(cx);
        cx.notify();
    }

    pub(in crate::features) fn set_ai_mcp_terminal_execution_mode(
        &mut self,
        mode: McpTerminalExecutionMode,
        cx: &mut Context<Self>,
    ) {
        self.ai.set_settings_mcp_terminal_execution_mode(mode);
        self.persist_ai_settings_now(cx);
        cx.notify();
    }

    fn reconfigure_mcp_host_and_report(&mut self) {
        if let Err(error) = self.reconfigure_mcp_host() {
            tracing::warn!(error = %error, "MCP Host reconfiguration failed");
            self.ai
                .set_panel_status("MCP Host could not be started with the new settings");
        }
    }
}
