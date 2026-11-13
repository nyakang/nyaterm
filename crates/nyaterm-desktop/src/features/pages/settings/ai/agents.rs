use super::super::{settings_form_row, settings_switch};
use super::{ai_card, ai_field};
use crate::features::mcp::McpHostStatus;
use crate::features::pages::settings::panel::SettingsPanel;
use gpui::{Context, IntoElement, div, prelude::*, px, rgb};
use nyaterm_core::{AiPermissionMode, CodexThreadMode, McpTerminalDisplayMode};
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
        let mut codex_options = vec![NyaSelectOption::new(
            "__none__",
            t!("ai.useCodexDefaultModel"),
        )];
        codex_options.extend(
            status
                .codex_models
                .iter()
                .map(|name| NyaSelectOption::new(name.clone(), name.clone())),
        );
        if let Some(name) = &settings.codex.default_model
            && !status.codex_models.contains(name)
        {
            codex_options.push(NyaSelectOption::new(name.clone(), name.clone()));
        }
        let codex_model = self.form_select_control(
            "ai-codex-default-model",
            codex_options,
            Some(
                settings
                    .codex
                    .default_model
                    .clone()
                    .unwrap_or_else(|| "__none__".into()),
            ),
            status.pending,
            cx,
        );
        let thread = self.form_select_control(
            "ai-codex-thread-mode",
            vec![
                NyaSelectOption::new("persistent", t!("ai.codex.persistent")),
                NyaSelectOption::new("ephemeral", t!("ai.codex.ephemeral")),
            ],
            Some(
                if settings.codex.thread_mode == CodexThreadMode::Persistent {
                    "persistent"
                } else {
                    "ephemeral"
                }
                .into(),
            ),
            false,
            cx,
        );
        let codex_permission = self.form_select_control(
            "ai-codex-permission",
            permissions(),
            Some(permission(&settings.codex.permission_mode)),
            false,
            cx,
        );
        let codex_header = div()
            .flex()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap_3()
            .child(
                div().flex().flex_col().gap_1().child("OpenAI Codex").child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(palette.text_muted))
                        .child(t!("ai.codexDesc")),
                ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(agent_badge(
                        palette,
                        status.codex_version.is_some(),
                        t!("ai.installed"),
                        t!("ai.notInstalled"),
                    ))
                    .child(agent_badge(
                        palette,
                        status.codex_connected,
                        t!("ai.connected"),
                        t!("ai.disconnected"),
                    ))
                    .child(settings_switch(
                        palette,
                        "ai-codex-enabled",
                        settings.codex.enabled,
                        cx.listener(|panel, _, _, cx| panel.toggle_ai_codex_enabled(cx)),
                    )),
            );
        let mut codex = div()
            .rounded_md()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(rgb(palette.bg))
            .p_4()
            .flex()
            .flex_col()
            .gap_4()
            .child(codex_header)
            .child(ai_field(
                palette,
                t!("ai.codexPath"),
                None,
                self.existing_text_input_box("ai.input.codex-executable", false),
            ))
            .child(
                div()
                    .grid()
                    .grid_cols(columns)
                    .gap_4()
                    .child(
                        ai_field(palette, t!("ai.codex.threadMode"), None, thread)
                            .debug_selector(|| "ai-agent-thread-field".into()),
                    )
                    .child(
                        ai_field(palette, t!("ai.codexDefaultModel"), None, codex_model)
                            .debug_selector(|| "ai-agent-model-field".into()),
                    )
                    .child(ai_field(
                        palette,
                        t!("ai.permission.title"),
                        None,
                        codex_permission,
                    )),
            )
            .child(
                div()
                    .grid()
                    .grid_cols(2)
                    .gap_2()
                    .text_size(px(12.))
                    .text_color(rgb(palette.text_muted))
                    .child(format!(
                        "{}: {}",
                        t!("ai.codexVersion"),
                        status.codex_version.as_deref().unwrap_or("-")
                    ))
                    .child(format!(
                        "{}: {}",
                        t!("ai.codexAuthMode"),
                        status.codex_auth_mode.as_deref().unwrap_or("-")
                    ))
                    .child(format!(
                        "{}: {}",
                        t!("ai.codexPlan"),
                        status.codex_plan.as_deref().unwrap_or("-")
                    ))
                    .child(format!(
                        "{}: {}",
                        t!("ai.codexEmail"),
                        status.codex_email.as_deref().unwrap_or("-")
                    )),
            );
        if let Some(url) = &status.verification_url {
            codex = codex.child(
                div()
                    .rounded_md()
                    .bg(rgb(palette.surface))
                    .p_3()
                    .child(url.clone())
                    .child(status.user_code.clone().unwrap_or_default()),
            );
        }
        codex = codex.child(
            div()
                .flex()
                .flex_wrap()
                .gap_2()
                .child(
                    NyaButton::new("ai-codex-status", t!("ai.refreshStatus"))
                        .small()
                        .disabled(status.pending)
                        .on_click(cx.listener(|panel, _, _, cx| panel.refresh_codex_account(cx))),
                )
                .child(
                    NyaButton::new("ai-codex-login", t!("ai.codexLogin"))
                        .small()
                        .disabled(status.pending)
                        .on_click(
                            cx.listener(|panel, _, _, cx| panel.start_codex_login(false, cx)),
                        ),
                )
                .child(
                    NyaButton::new("ai-codex-device", t!("ai.codexDeviceCodeLogin"))
                        .small()
                        .disabled(status.pending)
                        .on_click(cx.listener(|panel, _, _, cx| panel.start_codex_login(true, cx))),
                )
                .when(status.login_id.is_some(), |row| {
                    row.child(
                        NyaButton::new("ai-codex-cancel", t!("ai.cancelLogin"))
                            .small()
                            .on_click(cx.listener(|panel, _, _, cx| panel.cancel_codex_login(cx))),
                    )
                })
                .when(status.user_code.is_some(), |row| {
                    row.child(
                        NyaButton::new("ai-codex-copy", t!("ai.copyCode"))
                            .small()
                            .on_click(
                                cx.listener(|panel, _, _, cx| panel.copy_codex_device_code(cx)),
                            ),
                    )
                })
                .child(
                    NyaButton::new("ai-codex-models", t!("ai.refreshModels"))
                        .small()
                        .disabled(status.pending)
                        .on_click(cx.listener(|panel, _, _, cx| {
                            panel.with_app(cx, |app, cx| app.refresh_codex_models(cx));
                        })),
                )
                .child(
                    NyaButton::new("ai-codex-logout", t!("ai.codexLogout"))
                        .small()
                        .disabled(status.pending)
                        .on_click(cx.listener(|panel, _, _, cx| panel.logout_codex(cx))),
                ),
        );
        let claude_permission = self.form_select_control(
            "ai-claude-permission",
            permissions(),
            Some(permission(&settings.claude_code.permission_mode)),
            false,
            cx,
        );
        let claude = div()
            .rounded_md()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(rgb(palette.bg))
            .p_4()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(
                        div().flex().flex_col().gap_1().child("Claude Code").child(
                            div()
                                .text_size(px(12.))
                                .text_color(rgb(palette.text_muted))
                                .child(t!("ai.claudeCodeDesc")),
                        ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(agent_badge(
                                palette,
                                status.claude_version.is_some(),
                                t!("ai.installed"),
                                t!("ai.notInstalled"),
                            ))
                            .child(agent_badge(
                                palette,
                                status.claude_connected,
                                t!("ai.connected"),
                                t!("ai.disconnected"),
                            ))
                            .child(settings_switch(
                                palette,
                                "ai-claude-enabled",
                                settings.claude_code.enabled,
                                cx.listener(|panel, _, _, cx| panel.toggle_ai_claude_enabled(cx)),
                            )),
                    ),
            )
            .child(ai_field(
                palette,
                t!("ai.claudeCodePath"),
                None,
                self.existing_text_input_box("ai.input.claude-executable", false),
            ))
            .child(
                div()
                    .grid()
                    .grid_cols(columns)
                    .gap_4()
                    .child(ai_field(
                        palette,
                        t!("ai.claudeCodeDefaultModel"),
                        None,
                        self.existing_text_input_box("ai.input.claude-default-model", false),
                    ))
                    .child(ai_field(
                        palette,
                        t!("ai.permission.title"),
                        None,
                        claude_permission,
                    )),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(palette.text_muted))
                    .child(format!(
                        "{}: {}",
                        t!("ai.claudeCodeVersion"),
                        status.claude_version.as_deref().unwrap_or("-")
                    )),
            )
            .child(
                div().flex().flex_wrap().gap_2().child(
                    NyaButton::new("ai-claude-status", t!("ai.refreshClaudeStatus"))
                        .small()
                        .disabled(status.pending)
                        .on_click(cx.listener(|panel, _, _, cx| panel.refresh_claude_account(cx))),
                ),
            );
        let detect = NyaButton::new("ai-agents-detect", t!("ai.detect"))
            .small()
            .icon("icons/fe/refresh.svg")
            .disabled(status.pending)
            .on_click(cx.listener(|panel, _, _, cx| panel.refresh_ai_agents(cx)));
        let local = div()
            .flex()
            .flex_col()
            .gap_4()
            .child(codex)
            .child(claude)
            .when_some(status.error, |body, error| {
                body.child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(palette.warning))
                        .child(nyaterm_core::sanitize_ai_diagnostic(&error, 500)),
                )
            });
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
        let helper_label = if helper_ready {
            t!("ai.mcp.helperAvailable")
        } else {
            t!("ai.mcp.helperMissing")
        };
        let mcp = div()
            .child(self.mcp_background_tasks_content(cx))
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(palette.text_muted))
                    .child(helper_label),
            )
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
                        t!("ai.mcp.terminalDisplay"),
                        None,
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
                                .small()
                                .full_width()
                                .on_click(cx.listener(
                                    move |panel, _, _, cx| {
                                        panel.copy_external_mcp_config(client, cx)
                                    },
                                )),
                            )
                    }),
                ),
            );
        let advanced_open = self.ai.providers.advanced_open;
        let advanced_button = NyaButton::new("ai-advanced-toggle", t!("settings.advanced"))
            .full_width()
            .on_click(cx.listener(|panel, _, _, cx| {
                panel.with_app(cx, |app, cx| app.toggle_ai_advanced(cx));
            }));
        let mut advanced = div().child(advanced_button);
        if advanced_open {
            let default_agent = self.form_select_control(
                "ai-default-agent",
                vec![
                    NyaSelectOption::new("nyaterm", "NyaTerm"),
                    NyaSelectOption::new("codex", "Codex"),
                    NyaSelectOption::new("claude_code", "Claude Code"),
                ],
                Some(
                    match settings.default_agent_kind {
                        nyaterm_core::AiAgentKind::Nyaterm => "nyaterm",
                        nyaterm_core::AiAgentKind::Codex => "codex",
                        nyaterm_core::AiAgentKind::ClaudeCode => "claude_code",
                    }
                    .into(),
                ),
                false,
                cx,
            );
            let external_permission = self.form_select_control(
                "ai-external-permission",
                permissions(),
                Some(permission(&settings.external_agent_permission_mode)),
                false,
                cx,
            );
            advanced = advanced.child(
                div()
                    .pt_4()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(ai_field(
                        palette,
                        t!("ai.defaultAgent"),
                        None,
                        default_agent,
                    ))
                    .child(ai_field(
                        palette,
                        t!("ai.externalPermission"),
                        None,
                        external_permission,
                    ))
                    .child(ai_field(
                        palette,
                        format!("Codex · {}", t!("ai.configDirectory")),
                        None,
                        self.existing_text_input_box("ai.input.codex-config-directory", false),
                    ))
                    .child(ai_field(
                        palette,
                        format!("Claude Code · {}", t!("ai.configDirectory")),
                        None,
                        self.existing_text_input_box("ai.input.claude-config-directory", false),
                    ))
                    .child(settings_form_row(
                        palette,
                        "Codex MCP",
                        None,
                        settings_switch(
                            palette,
                            "ai-codex-mcp-integration",
                            settings.codex.tool_integration_mode.as_deref() == Some("nyaterm_mcp"),
                            cx.listener(|panel, _, _, cx| {
                                panel.toggle_ai_codex_mcp_integration(cx)
                            }),
                        ),
                    ))
                    .child(settings_form_row(
                        palette,
                        "Claude Code MCP",
                        None,
                        settings_switch(
                            palette,
                            "ai-claude-mcp-integration",
                            settings.claude_code.tool_integration_mode.as_deref()
                                == Some("nyaterm_mcp"),
                            cx.listener(|panel, _, _, cx| {
                                panel.toggle_ai_claude_mcp_integration(cx)
                            }),
                        ),
                    )),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap_5()
            .child(ai_card(
                palette,
                div().child(t!("ai.localAgents")),
                detect,
                local,
            ))
            .child(ai_card(
                palette,
                div().child(t!("ai.externalMcp")),
                div(),
                mcp,
            ))
            .child(advanced)
    }
}
fn agent_badge(
    palette: crate::theme::ThemePalette,
    active: bool,
    yes: impl Into<gpui::SharedString>,
    no: impl Into<gpui::SharedString>,
) -> impl IntoElement {
    div()
        .rounded_md()
        .border_1()
        .border_color(rgb(palette.border))
        .px_2()
        .py_1()
        .text_size(px(11.))
        .text_color(rgb(if active {
            palette.success
        } else {
            palette.text_muted
        }))
        .child(if active { yes.into() } else { no.into() })
}
