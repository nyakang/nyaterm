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
        let http_view = self.http_mcp_view(cx);
        let is_http = settings.external_mcp.transport
            == nyaterm_core::ai::mcp_http::McpTransport::StreamableHttp;
        let transport = self.form_select_control(
            "ai-mcp-transport",
            vec![
                NyaSelectOption::new("stdio", "stdio"),
                NyaSelectOption::new("streamable_http", "Streamable HTTP"),
            ],
            Some(settings.external_mcp.transport.as_str().to_owned()),
            false,
            cx,
        );
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
        let mcp_status = if matches!(
            settings.external_mcp.transport,
            nyaterm_core::ai::mcp_http::McpTransport::Unknown(_)
        ) {
            t!("ai.mcp.unsupportedTransport")
        } else if is_http {
            t!(http_view.status_key)
        } else {
            match self.mcp_host_status(cx) {
                McpHostStatus::Running => t!("ai.mcp.running"),
                McpHostStatus::Unavailable => t!("ai.mcp.unavailable"),
                _ => t!("ai.mcp.disabled"),
            }
        };
        let helper_ready = status.mcp_helper_path.is_some();
        let permission_hint_key = format!(
            "ai.mcp.permissionHints.{}",
            permission(&settings.external_mcp.permission_mode)
        );
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
            .child(ai_field(palette, t!("ai.mcp.transport"), None, transport))
            .when(
                settings.external_mcp.permission_mode == AiPermissionMode::FullAccess,
                |this| {
                    this.child(
                        div()
                            .id("mcp-full-access-warning")
                            .text_size(px(12.))
                            .text_color(rgb(palette.danger))
                            .child(t!("ai.mcp.fullAccessPersistent")),
                    )
                },
            )
            .when(is_http, |this| {
                this.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(self.existing_text_input_field(
                            "ai.mcp.http-port",
                            t!("ai.mcp.httpPort"),
                            false,
                        ))
                        .child(self.existing_text_input_field(
                            "ai.mcp.http-origins",
                            t!("ai.mcp.httpOrigins"),
                            true,
                        ))
                        .child(div().text_sm().child(t!("ai.mcp.httpTokenHint")))
                        .child(div().text_sm().child(format!(
                            "{} {}",
                            t!("ai.mcp.httpOwner"),
                            if http_view.owned_by_current_window {
                                t!("ai.mcp.currentWindow").to_string()
                            } else if http_view.owner.is_some() {
                                t!("ai.mcp.httpOtherWindow").to_string()
                            } else {
                                "—".into()
                            }
                        )))
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .child(
                                    NyaButton::new("mcp-http-apply", t!("ai.mcp.httpApply"))
                                        .on_click(cx.listener(|panel, _, _, cx| {
                                            panel
                                                .with_app(cx, |app, cx| app.apply_http_mcp_form(cx))
                                        })),
                                )
                                .child(
                                    NyaButton::new("mcp-http-claim", t!("ai.mcp.httpUseWindow"))
                                        .on_click(cx.listener(|panel, _, _, cx| {
                                            panel.with_app(cx, |app, cx| {
                                                app.schedule_http_mcp(true, cx)
                                            })
                                        })),
                                ),
                        ),
                )
            })
            .child(
                div()
                    .grid()
                    .grid_cols(columns)
                    .gap_4()
                    .child(ai_field(
                        palette,
                        t!("ai.permission.title"),
                        Some(t!(&permission_hint_key).into()),
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
            .child(self.mcp_setup_content(cx))
            .child(self.mcp_activity_content(cx));
        div().flex().flex_col().gap_5().child(local).child(ai_card(
            palette,
            div().child(t!("ai.externalMcp")),
            div(),
            mcp,
        ))
    }
}
