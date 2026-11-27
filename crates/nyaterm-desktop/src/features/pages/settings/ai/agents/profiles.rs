use crate::features::ai::agent_management::AgentManagementView;
use crate::features::ai::agent_settings::{default_selectable, execution_unavailable_key};
use crate::features::pages::settings::ai::{ai_card, ai_field};
use crate::features::pages::settings::panel::SettingsPanel;
use crate::features::pages::settings::settings_form_row;
use gpui::{Context, div, prelude::*, px, rgb};
use nyaterm_core::AiSettings;
use nyaterm_core::ai::external_agents::{ExternalAgentProfile, profile_id, profiles};
use nyaterm_ui::{NyaButton, NyaButtonVariant, NyaSelectOption, NyaSwitch};
use rust_i18n::t;

impl SettingsPanel {
    pub(super) fn ai_agent_profiles_content(
        &mut self,
        settings: &AiSettings,
        status: &AgentManagementView,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let palette = self.theme_palette();
        let selected = profile_id(&settings.default_agent_kind).unwrap_or("nyaterm");
        let default_agent = self.form_select_control(
            "ai-default-agent",
            std::iter::once(NyaSelectOption::new("nyaterm", "NyaTerm"))
                .chain(profiles(settings).iter().map(|profile| {
                    let available = default_selectable(profile)
                        && !self.ai.agent_editor.errors.contains_key(&profile.id);
                    let reason = execution_unavailable_key(profile).unwrap_or(if profile.enabled {
                        "ai.acp.invalidProfile"
                    } else {
                        "ai.acp.disabledStatus"
                    });
                    NyaSelectOption::new(profile.id.clone(), profile.name.clone())
                        .disabled(!available)
                        .subtitle(if available {
                            t!("ai.acp.enabledStatus")
                        } else {
                            t!(reason)
                        })
                }))
                .collect(),
            Some(selected.into()),
            false,
            cx,
        );
        let mut content = div()
            .flex()
            .flex_col()
            .gap_4()
            .child(ai_field(
                palette,
                t!("ai.defaultAgent"),
                Some(t!("ai.acp.defaultHint").into()),
                default_agent,
            ))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(palette.text_muted))
                    .child(t!("ai.acp.draftHint")),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(palette.text_muted))
                    .child(t!("ai.acp.installHint")),
            );
        if profiles(settings).is_empty() {
            content = content.child(div().py_4().child(t!("ai.acp.empty")));
        }
        let mut actions = div().flex().flex_wrap().gap_2().child(
            NyaButton::new("acp-refresh", t!("ai.acp.refresh"))
                .small()
                .loading(status.pending)
                .disabled(status.pending)
                .on_click(cx.listener(|panel, _, _, cx| panel.refresh_ai_agents(cx))),
        );
        if status.pending {
            actions = actions.child(
                NyaButton::new("acp-cancel-inspect", t!("ai.acp.cancel"))
                    .small()
                    .on_click(cx.listener(|panel, _, _, cx| {
                        panel.with_app(cx, |app, cx| {
                            app.ai.cancel_agent_management();
                            app.request_settings_panel_refresh(cx);
                        });
                    })),
            );
        }
        for (strategy, label) in [("claude", "+ Claude Code"), ("codex", "+ Codex")] {
            actions = actions.child(
                NyaButton::new(format!("acp-add-{strategy}"), label)
                    .small()
                    .tooltip(t!(if strategy == "codex" {
                        "ai.acp.detectionOnly"
                    } else {
                        "ai.acp.installHint"
                    }))
                    .on_click(cx.listener(move |panel, _, _, cx| {
                        panel.with_app(cx, |app, cx| {
                            app.edit_acp_profile("", "add", strategy.into(), cx)
                        });
                    })),
            );
        }
        content = content.child(actions);
        for profile in profiles(settings) {
            content = content.child(self.ai_agent_profile_card(profile, status, cx));
        }
        if let Some(error) = &status.error {
            content = content.child(div().text_color(rgb(palette.warning)).child(error.clone()));
        }
        // Unknown strategies remain editable for compatibility, but are not offered as runnable agents.
        ai_card(palette, div().child(t!("ai.localAgents")), div(), content)
    }

    fn ai_agent_profile_card(
        &mut self,
        profile: &ExternalAgentProfile,
        status: &AgentManagementView,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let palette = self.theme_palette();
        let id = profile.id.clone();
        let editor = self.ai.agent_editor.clone();
        let advanced = editor.advanced.contains(&id);
        let unavailable = execution_unavailable_key(profile);
        let busy = status.pending && status.active_profile.as_deref() == Some(id.as_str());
        let enable_id = id.clone();
        let mut card = div()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .border_1()
            .border_color(rgb(palette.border))
            .rounded_md()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(div().text_size(px(14.)).child(profile.name.clone()))
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(if unavailable.is_some() {
                                        palette.warning
                                    } else {
                                        palette.text_muted
                                    }))
                                    .child(t!(unavailable.unwrap_or(if profile.enabled {
                                        "ai.acp.enabledStatus"
                                    } else {
                                        "ai.acp.disabledStatus"
                                    }))),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_size(px(12.))
                            .child(t!("ai.acp.enable"))
                            .child(
                                NyaSwitch::new(format!("acp-{id}-enabled"))
                                    .checked(profile.enabled)
                                    .disabled(!profile.enabled && unavailable.is_some())
                                    .on_click(cx.listener(move |panel, _, _, cx| {
                                        panel.with_app(cx, |app, cx| {
                                            app.edit_acp_profile(
                                                &enable_id,
                                                "enabled",
                                                String::new(),
                                                cx,
                                            )
                                        });
                                    })),
                            ),
                    ),
            );
        let columns = if self.content_width >= 1024. { 2 } else { 1 };
        let mut basic = div().grid().grid_cols(columns).gap_3();
        for (field, label, hint) in [
            ("name", "ai.acp.name", None),
            (
                "executable",
                "ai.acp.executable",
                Some("ai.acp.executableHint"),
            ),
            ("model", "ai.acp.model", Some("ai.acp.modelHint")),
        ] {
            basic = basic.child(self.ai_agent_field(&id, field, label, hint));
        }
        card = card.child(basic);
        let advanced_id = id.clone();
        let inspect_id = id.clone();
        let copy_id = id.clone();
        let delete_id = id.clone();
        card = card.child(
            div()
                .flex()
                .flex_wrap()
                .gap_2()
                .child(
                    NyaButton::new(
                        format!("acp-inspect-{id}"),
                        t!(if busy {
                            "ai.acp.checking"
                        } else {
                            "ai.acp.check"
                        }),
                    )
                    .small()
                    .variant(NyaButtonVariant::Primary)
                    .loading(busy)
                    .disabled(
                        status.pending
                            || !cfg!(windows)
                            || !profile.strategy.supported()
                            || editor.errors.contains_key(&id),
                    )
                    .on_click(cx.listener(move |panel, _, _, cx| {
                        panel.with_app(cx, |app, cx| app.inspect_acp_profile(&inspect_id, cx));
                    })),
                )
                .child(
                    NyaButton::new(
                        format!("acp-advanced-{id}"),
                        t!(if advanced {
                            "ai.acp.hideAdvanced"
                        } else {
                            "ai.acp.advanced"
                        }),
                    )
                    .small()
                    .on_click(cx.listener(move |panel, _, _, cx| {
                        panel.with_app(cx, |app, cx| {
                            app.edit_acp_profile(&advanced_id, "advanced", String::new(), cx)
                        });
                    })),
                )
                .child(
                    NyaButton::new(format!("acp-{id}-copy"), t!("ai.acp.copy"))
                        .small()
                        .on_click(cx.listener(move |panel, _, _, cx| {
                            panel.with_app(cx, |app, cx| {
                                app.edit_acp_profile(&copy_id, "copy", String::new(), cx)
                            });
                        })),
                )
                .child(
                    NyaButton::new(format!("acp-{id}-delete"), t!("ai.acp.delete"))
                        .small()
                        .variant(NyaButtonVariant::Ghost)
                        .on_click(cx.listener(move |panel, _, _, cx| {
                            panel.with_app(cx, |app, cx| {
                                app.edit_acp_profile(
                                    &delete_id,
                                    "request-delete",
                                    String::new(),
                                    cx,
                                )
                            });
                        })),
                ),
        );
        if editor.delete_pending.as_deref() == Some(id.as_str()) {
            let remove_id = id.clone();
            let cancel_id = id.clone();
            card = card.child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(div().text_size(px(12.)).child(t!("ai.acp.deleteHint")))
                    .child(
                        NyaButton::new(format!("acp-confirm-delete-{id}"), t!("ai.acp.delete"))
                            .small()
                            .variant(NyaButtonVariant::Danger)
                            .on_click(cx.listener(move |panel, _, _, cx| {
                                panel.with_app(cx, |app, cx| {
                                    app.edit_acp_profile(&remove_id, "delete", String::new(), cx)
                                });
                            })),
                    )
                    .child(
                        NyaButton::new(format!("acp-cancel-delete-{id}"), t!("common.cancel"))
                            .small()
                            .on_click(cx.listener(move |panel, _, _, cx| {
                                panel.with_app(cx, |app, cx| {
                                    app.edit_acp_profile(
                                        &cancel_id,
                                        "cancel-delete",
                                        String::new(),
                                        cx,
                                    )
                                });
                            })),
                    ),
            );
        }
        if advanced {
            card = card.child(self.ai_agent_advanced_fields(profile, columns, cx));
        }
        if let Some(error) = editor.errors.get(&id) {
            card = card.child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(palette.warning))
                    .child(t!(*error)),
            );
        } else if editor.dirty.contains(&id) {
            card = card.child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(palette.text_muted))
                    .child(t!("settings.unappliedChanges")),
            );
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
                            });
                        })),
                );
            }
            if state.logout {
                let id = id.clone();
                account = account.child(
                    NyaButton::new(format!("acp-logout-{id}"), t!("ai.acp.logout"))
                        .small()
                        .disabled(status.pending)
                        .on_click(cx.listener(move |panel, _, _, cx| {
                            panel.with_app(cx, |app, cx| app.acp_account_action(&id, None, cx));
                        })),
                );
            }
            card = card.child(account).child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(if state.error.is_some() {
                        palette.warning
                    } else {
                        palette.text_muted
                    }))
                    .child(state.error.clone().unwrap_or_else(|| {
                        t!(match state.authenticated {
                            Some(true) => "ai.acp.connected",
                            Some(false) => "ai.acp.loginRequired",
                            None => "ai.acp.authUnknown",
                        })
                        .into()
                    })),
            );
        } else if !busy {
            card = card.child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(palette.text_muted))
                    .child(t!("ai.acp.notChecked")),
            );
        }
        card
    }

    fn ai_agent_field(&self, id: &str, field: &str, label: &str, hint: Option<&str>) -> gpui::Div {
        let selector = format!("acp-profile-field-{id}-{field}");
        div()
            .min_w_0()
            .debug_selector(move || selector.clone())
            .child(ai_field(
                self.theme_palette(),
                t!(label),
                hint.map(|key| t!(key).into()),
                self.existing_text_input_box(format!("ai.agent.{id}.{field}"), false),
            ))
    }

    fn ai_agent_advanced_fields(
        &mut self,
        profile: &ExternalAgentProfile,
        columns: u16,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let palette = self.theme_palette();
        let id = &profile.id;
        let mut strategies = vec![
            NyaSelectOption::new("claude", "Claude Code"),
            NyaSelectOption::new("codex", "Codex"),
        ];
        if !profile.strategy.supported() {
            strategies.push(
                NyaSelectOption::new(profile.strategy.0.clone(), profile.strategy.0.clone())
                    .disabled(true),
            );
        }
        let strategy = self.form_select_control(
            format!("ai-agent-strategy-{id}"),
            strategies,
            Some(profile.strategy.0.clone()),
            false,
            cx,
        );
        let mut fields = div().grid().grid_cols(columns).gap_3().child(ai_field(
            palette,
            t!("ai.acp.strategy"),
            None,
            strategy,
        ));
        for (field, label, hint) in [
            ("cwd", "ai.acp.cwd", Some("ai.acp.directoryHint")),
            ("config", "ai.acp.config", Some("ai.acp.directoryHint")),
            ("args", "ai.acp.args", Some("ai.acp.argsUnsupported")),
        ] {
            fields = fields.child(self.ai_agent_field(id, field, label, hint));
        }
        let persistent_id = id.clone();
        let mut advanced = div()
            .flex()
            .flex_col()
            .gap_3()
            .pt_3()
            .border_t_1()
            .border_color(rgb(palette.border))
            .child(fields)
            .child(settings_form_row(
                palette,
                t!("ai.acp.persistent"),
                Some(t!("ai.acp.persistentHint").into()),
                NyaSwitch::new(format!("acp-{id}-persistent"))
                    .checked(profile.persistent)
                    .on_click(cx.listener(move |panel, _, _, cx| {
                        panel.with_app(cx, |app, cx| {
                            app.edit_acp_profile(&persistent_id, "persistent", String::new(), cx)
                        });
                    })),
            ));
        for env in &profile.environment {
            let row_id = env.id.clone();
            let delete_id = id.clone();
            advanced = advanced.child(
                div()
                    .flex()
                    .items_end()
                    .gap_2()
                    .child(div().flex_1().min_w_0().child(ai_field(
                        palette,
                        t!("ai.acp.envName"),
                        None,
                        self.existing_text_input_box(
                            format!("ai.agent.{id}.env.{row_id}.name"),
                            false,
                        ),
                    )))
                    .child(div().flex_1().min_w_0().child(ai_field(
                        palette,
                        t!("ai.acp.envValue"),
                        None,
                        self.existing_text_input_box(
                            format!("ai.agent.{id}.env.{row_id}.value"),
                            false,
                        ),
                    )))
                    .child(
                        NyaButton::new(format!("acp-env-remove-{row_id}"), t!("ai.acp.delete"))
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
        let add_id = id.clone();
        advanced.child(
            NyaButton::new(format!("acp-{id}-add-env"), t!("ai.acp.addEnv"))
                .small()
                .on_click(cx.listener(move |panel, _, _, cx| {
                    panel.with_app(cx, |app, cx| {
                        app.edit_acp_profile(&add_id, "add-env", String::new(), cx)
                    });
                })),
        )
    }
}
