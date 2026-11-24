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
        let mut local = div()
            .flex()
            .flex_col()
            .gap_4()
            .child(div().text_size(px(12.)).child(t!("ai.acp.installHint")));
        for profile in nyaterm_core::ai::external_agents::profiles(&settings) {
            let id = profile.id.clone();
            let mut card = div()
                .flex()
                .flex_col()
                .gap_3()
                .p_3()
                .border_1()
                .border_color(rgb(palette.border))
                .rounded_md()
                .child(div().text_size(px(14.)).child(profile.name.clone()));
            let mut fields = div().grid().grid_cols(columns).gap_3();
            for (field, label) in [
                ("name", t!("ai.acp.name")),
                ("strategy", t!("ai.acp.strategy")),
                ("executable", t!("ai.acp.executable")),
                ("args", t!("ai.acp.args")),
                ("cwd", t!("ai.acp.cwd")),
                ("config", t!("ai.acp.config")),
                ("model", t!("ai.acp.model")),
            ] {
                let selector = format!("acp-profile-field-{id}-{field}");
                fields = fields.child(
                    div()
                        .min_w_0()
                        .debug_selector(move || selector.clone())
                        .child(ai_field(
                            palette,
                            label,
                            None,
                            self.existing_text_input_box(format!("ai.agent.{id}.{field}"), false),
                        )),
                );
            }
            card = card.child(fields);
            for variable in &profile.environment {
                let row_id = variable.id.clone();
                let delete_id = id.clone();
                card = card.child(
                    div()
                        .flex()
                        .gap_2()
                        .child(div().flex_1().min_w_0().child(self.existing_text_input_box(
                            format!("ai.agent.{id}.env.{row_id}.name"),
                            false,
                        )))
                        .child(div().flex_1().min_w_0().child(self.existing_text_input_box(
                            format!("ai.agent.{id}.env.{row_id}.value"),
                            false,
                        )))
                        .child(
                            NyaButton::new(format!("acp-env-remove-{row_id}"), "−")
                                .small()
                                .on_click(cx.listener(move |panel, _, _, cx| {
                                    panel.with_app(cx, |app, cx| {
                                        app.edit_acp_profile(
                                            &delete_id,
                                            &format!("env.{row_id}.delete"),
                                            String::new(),
                                            cx,
                                        )
                                    });
                                })),
                        ),
                );
            }
            let mut actions = div().flex().flex_wrap().gap_2();
            for (action, label) in [
                (
                    "enabled",
                    if profile.enabled {
                        t!("ai.acp.disable")
                    } else {
                        t!("ai.acp.enable")
                    },
                ),
                (
                    "persistent",
                    if profile.persistent {
                        t!("ai.acp.persistentOn")
                    } else {
                        t!("ai.acp.persistentOff")
                    },
                ),
                ("add-env", t!("ai.acp.addEnv")),
                ("copy", t!("ai.acp.copy")),
                ("delete", t!("ai.acp.delete")),
            ] {
                let id = id.clone();
                actions = actions.child(
                    NyaButton::new(format!("acp-{id}-{action}"), label)
                        .small()
                        .disabled(
                            action == "enabled"
                                && !profile.enabled
                                && nyaterm_agent_client::acp::policy::admit_execution(profile)
                                    .is_err(),
                        )
                        .on_click(cx.listener(move |panel, _, _, cx| {
                            panel.with_app(cx, |app, cx| {
                                app.edit_acp_profile(&id, action, String::new(), cx)
                            })
                        })),
                );
            }
            let inspect_id = id.clone();
            actions = actions.child(
                NyaButton::new(format!("acp-inspect-{id}"), t!("ai.acp.check"))
                    .small()
                    .disabled(status.pending)
                    .on_click(cx.listener(move |panel, _, _, cx| {
                        panel.with_app(cx, |app, cx| app.inspect_acp_profile(&inspect_id, cx))
                    })),
            );
            card = card.child(actions);
            if nyaterm_agent_client::acp::policy::admit_execution(profile).is_err() {
                card = card.child(div().text_size(px(12.)).child(t!("ai.acp.blocked")));
            }
            if let Some(state) = status.profiles.get(&id) {
                let mut account = div().flex().flex_wrap().gap_2();
                for (method, label) in &state.auth_methods {
                    let (id, method) = (id.clone(), method.clone());
                    account = account.child(
                        NyaButton::new(format!("acp-login-{id}-{method}"), label.clone())
                            .small()
                            .disabled(status.pending)
                            .on_click(cx.listener(move |panel, _, _, cx| {
                                panel.with_app(cx, |app, cx| {
                                    app.acp_account_action(&id, Some(method.clone()), cx)
                                })
                            })),
                    );
                }
                if state.logout {
                    let logout_id = id.clone();
                    account = account.child(
                        NyaButton::new(format!("acp-logout-{id}"), t!("ai.acp.logout"))
                            .small()
                            .disabled(status.pending)
                            .on_click(cx.listener(move |panel, _, _, cx| {
                                panel.with_app(cx, |app, cx| {
                                    app.acp_account_action(&logout_id, None, cx)
                                })
                            })),
                    );
                }
                card = card.child(account);

                card =
                    card.child(
                        div()
                            .text_size(px(12.))
                            .child(state.error.clone().unwrap_or_else(
                                || match state.authenticated {
                                    Some(true) => t!("ai.acp.connected").into(),
                                    Some(false) => t!("ai.acp.loginRequired").into(),
                                    None => t!("ai.acp.authUnknown").into(),
                                },
                            )),
                    );
            }
            local = local.child(card);
        }
        if let Some(error) = status.error.clone() {
            local = local.child(div().text_color(rgb(palette.warning)).child(error));
        }
        let mut detect = div()
            .flex()
            .flex_wrap()
            .gap_2()
            .child(
                NyaButton::new("acp-refresh", t!("ai.acp.refresh"))
                    .small()
                    .disabled(status.pending)
                    .on_click(cx.listener(|panel, _, _, cx| panel.refresh_ai_agents(cx))),
            )
            .when(status.pending, |actions| {
                actions.child(
                    NyaButton::new("acp-cancel-inspect", t!("ai.acp.cancel"))
                        .small()
                        .on_click(cx.listener(|panel, _, _, cx| {
                            panel.with_app(cx, |app, cx| {
                                app.ai.cancel_agent_management();
                                app.request_settings_panel_refresh(cx);
                            })
                        })),
                )
            });

        for (strategy, label) in [
            ("codex", std::borrow::Cow::Borrowed("+ Codex")),
            ("claude", std::borrow::Cow::Borrowed("+ Claude")),
            ("custom", t!("ai.acp.addCustom")),
        ] {
            detect = detect.child(
                NyaButton::new(format!("acp-add-{strategy}"), label)
                    .small()
                    .on_click(cx.listener(move |panel, _, _, cx| {
                        panel.with_app(cx, |app, cx| {
                            app.edit_acp_profile("", "add", strategy.into(), cx)
                        })
                    })),
            );
        }
        let default_agent = self.form_select_control(
            "ai-default-agent",
            std::iter::once(NyaSelectOption::new("nyaterm", "NyaTerm"))
                .chain(
                    nyaterm_core::ai::external_agents::profiles(&settings)
                        .iter()
                        .map(|profile| {
                            NyaSelectOption::new(profile.id.clone(), profile.name.clone())
                        }),
                )
                .collect(),
            Some(
                nyaterm_core::ai::external_agents::profile_id(&settings.default_agent_kind)
                    .unwrap_or("nyaterm")
                    .into(),
            ),
            false,
            cx,
        );
        local = local.child(ai_field(
            palette,
            t!("ai.defaultAgent"),
            None,
            default_agent,
        ));
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
            .when(!helper_ready, |this| {
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
    }
}
