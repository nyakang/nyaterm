mod profiles;

use super::super::{settings_form_row, settings_switch};
use super::{ai_card, ai_field};
use crate::features::mcp::McpHostStatus;
use crate::features::pages::settings::panel::SettingsPanel;
use gpui::{Context, IntoElement, div, prelude::*, px, rgb};
use nyaterm_core::ai::mcp_execution::McpTerminalExecutionMode;
use nyaterm_core::{AiPermissionMode, McpTerminalDisplayMode};
use nyaterm_ui::{NyaButton, NyaSelectOption};
use rust_i18n::t;

fn permissions() -> Vec<NyaSelectOption> {
    [
        ("observer", "ai.permission.observer"),
        ("confirm", "ai.permission.confirm"),
        ("auto", "ai.permission.auto"),
        ("full_access", "ai.permission.fullAccess"),
    ]
    .into_iter()
    .map(|(value, label)| NyaSelectOption::new(value, t!(label)))
    .collect()
}
fn permission(mode: &AiPermissionMode) -> String {
    match mode {
        AiPermissionMode::Observer => "observer",
        AiPermissionMode::Confirm => "confirm",
        AiPermissionMode::Auto => "auto",
        AiPermissionMode::FullAccess => "full_access",
    }
    .into()
}

impl SettingsPanel {
    pub(super) fn ai_agents_content(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = self.theme_palette();
        let settings = self.ai.settings_config().clone();
        let status = self.ai.agent_management.clone();
        let columns = if self.content_width >= 1024. { 2 } else { 1 };
        let local = self.ai_agent_profiles_content(&settings, &status, cx);
        let mcp_permission = self.form_select_control(
            "ai-mcp-permission",
            permissions(),
            Some(permission(&settings.external_mcp.permission_mode)),
            false,
            cx,
        );
        let scope = self.form_select_control(
            "ai-mcp-session-scope",
            vec![
                NyaSelectOption::new("current_window", t!("ai.mcp.currentWindow")),
                NyaSelectOption::new("all_sessions", t!("ai.mcp.allSessions")),
            ],
            Some(
                match settings.external_mcp.session_scope {
                    nyaterm_core::ExternalMcpSessionScope::CurrentWindow => "current_window",
                    nyaterm_core::ExternalMcpSessionScope::AllSessions => "all_sessions",
                }
                .into(),
            ),
            false,
            cx,
        );
        let execution_mode = self.form_select_control(
            "ai-mcp-terminal-execution-mode",
            vec![
                NyaSelectOption::new("auto", t!("ai.mcp.executionAuto")),
                NyaSelectOption::new("inline", t!("ai.mcp.executionInline")),
                NyaSelectOption::new("background", t!("ai.mcp.executionBackground")),
            ],
            Some(
                match settings.external_mcp.terminal_execution_mode {
                    McpTerminalExecutionMode::Auto => "auto",
                    McpTerminalExecutionMode::Inline => "inline",
                    McpTerminalExecutionMode::Background => "background",
                }
                .into(),
            ),
            false,
            cx,
        );
        let display_mode = self.form_select_control(
            "ai-mcp-terminal-display-mode",
            vec![
                NyaSelectOption::new("inline", t!("ai.mcp.inline")),
                NyaSelectOption::new("silent", t!("ai.mcp.silent")),
            ],
            Some(
                match settings.external_mcp.terminal_display_mode {
                    McpTerminalDisplayMode::Inline => "inline",
                    McpTerminalDisplayMode::Silent => "silent",
                }
                .into(),
            ),
            false,
            cx,
        );
        let mcp_status = match self.mcp_host_status(cx) {
            McpHostStatus::Running => t!("ai.mcp.running"),
            McpHostStatus::Unavailable => t!("ai.mcp.unavailable"),
            _ => t!("ai.mcp.disabled"),
        };
        let helper_ready = status.mcp_helper_path.is_some();
        let mcp = div()
            .child(self.mcp_background_tasks_content(cx))
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(palette.text_muted))
                    .child(t!("ai.acp.mcpDirection")),
            )
            .when(!status.helper_checked, |this| {
                this.child(div().text_size(px(12.)).child(t!("ai.acp.helperChecking")))
            })
            .when(status.helper_checked && !helper_ready, |this| {
                this.child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(palette.warning))
                        .child(t!("ai.mcp.helperMissing")),
                )
            })
            .child(settings_form_row(
                palette,
                t!("ai.externalMcpEnabled"),
                Some(t!("ai.externalMcpDesc").into()),
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(mcp_status)
                    .child(settings_switch(
                        palette,
                        "ai-mcp-enabled",
                        settings.external_mcp.enabled,
                        cx.listener(|panel, _, _, cx| panel.toggle_ai_mcp_enabled(cx)),
                    )),
            ))
            .child(
                div()
                    .grid()
                    .grid_cols(columns)
                    .gap_4()
                    .child(ai_field(
                        palette,
                        t!("ai.permission.title"),
                        None,
                        mcp_permission,
                    ))
                    .child(ai_field(palette, t!("ai.mcp.sessionScope"), None, scope))
                    .child(ai_field(
                        palette,
                        t!("ai.mcp.terminalExecution"),
                        Some(
                            match settings.external_mcp.terminal_execution_mode {
                                McpTerminalExecutionMode::Auto => t!("ai.mcp.executionAutoHint"),
                                McpTerminalExecutionMode::Inline => {
                                    t!("ai.mcp.executionInlineHint")
                                }
                                McpTerminalExecutionMode::Background => {
                                    t!("ai.mcp.executionBackgroundHint")
                                }
                            }
                            .into(),
                        ),
                        execution_mode,
                    ))
                    .child(ai_field(
                        palette,
                        t!("ai.mcp.terminalDisplay"),
                        Some(t!("ai.mcp.terminalDisplayDesc").into()),
                        display_mode,
                    )),
            )
            .child(
                div().flex().flex_wrap().gap_3().children(
                    [
                        ("codex", "Codex"),
                        ("claude", "Claude Code"),
                        ("cursor", "Cursor"),
                    ]
                    .into_iter()
                    .map(|(client, label)| {
                        div()
                            .flex_1()
                            .min_w(px(120.))
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(palette.border))
                            .p_3()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(label)
                            .child(
                                NyaButton::new(
                                    format!("ai-mcp-copy-{client}"),
                                    t!("ai.externalMcpCopyConfig"),
                                )
                                .form_control()
                                .full_width()
                                .disabled(!helper_ready)
                                .on_click(cx.listener(
                                    move |panel, _, _, cx| {
                                        panel.copy_external_mcp_config(client, cx)
                                    },
                                )),
                            )
                    }),
                ),
            );
        div().flex().flex_col().gap_5().child(local).child(ai_card(
            palette,
            div().child(t!("ai.externalMcp")),
            div(),
            mcp,
        ))
    }
}
