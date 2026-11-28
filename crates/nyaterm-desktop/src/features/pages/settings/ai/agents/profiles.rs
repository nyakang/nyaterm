use crate::features::ai::agent_management::AgentManagementView;
use crate::features::ai::agent_settings::status::{ProfileAction, project};
use crate::features::ai::agent_settings::{default_selectable, execution_unavailable_key};
use crate::features::pages::settings::ai::{ai_card, ai_field};
use crate::features::pages::settings::panel::SettingsPanel;
use crate::features::pages::settings::settings_form_row;
use gpui::{Context, FontWeight, div, prelude::*, px, rgb};
use nyaterm_core::AiSettings;
use nyaterm_core::ai::external_agents::{
    AGENT_PRESETS, ExternalAgentProfile, profile_id, profiles,
};
use nyaterm_ui::{
    NyaButton, NyaButtonVariant, NyaDropdownMenu, NyaMenuItem, NyaSelectOption, NyaSwitch,
};
use rust_i18n::t;

impl SettingsPanel {
    pub(super) fn ai_agent_profiles_content(
        &mut self,
        settings: &AiSettings,
        status: &AgentManagementView,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let palette = self.theme_palette();
        self.agent_disclosure_focus
            .retain(|(id, _), _| profiles(settings).iter().any(|profile| profile.id == *id));
        let selected = profile_id(&settings.default_agent_kind).unwrap_or("nyaterm");
        let default_agent = self.form_select_control(
            "ai-default-agent",
            std::iter::once(NyaSelectOption::new("nyaterm", t!("ai.acp.builtin")))
                .chain(profiles(settings).iter().map(|profile| {
                    let available = default_selectable(profile)
                        && !self.ai.agent_editor.errors.contains_key(&profile.id);
                    let presentation = project(profile, &self.ai.agent_editor, status);
                    let summary = t!(presentation.message_key);
                    NyaSelectOption::new(profile.id.clone(), profile.name.clone())
                        .disabled(!available)
                        .subtitle(if profile.enabled {
                            summary.to_string()
                        } else {
                            format!("{} · {}", t!("ai.acp.disabledStatus"), summary)
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
                None,
                default_agent,
            ))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(palette.text_muted))
                    .child(t!("ai.acp.draftHint")),
            );
        if profiles(settings).is_empty() {
            content = content.child(div().py_4().child(t!("ai.acp.empty")));
        }
        let mut actions = div().flex().flex_wrap().gap_2().child(
            NyaButton::new("acp-refresh", t!("ai.acp.refresh"))
                .form_control()
                .loading(status.pending)
                .disabled(status.pending)
                .on_click(cx.listener(|panel, _, _, cx| panel.refresh_ai_agents(cx))),
        );
        if status.pending {
            actions = actions.child(
                NyaButton::new("acp-cancel-inspect", t!("ai.acp.cancel"))
                    .form_control()
                    .on_click(cx.listener(|panel, _, _, cx| {
                        panel.with_app(cx, |app, cx| {
                            app.ai.cancel_agent_management();
                            app.request_settings_panel_refresh(cx);
                        });
                    })),
            );
        }
        for preset in AGENT_PRESETS {
            let strategy = preset.strategy;
            let label = if strategy == "custom_acp" {
                t!("ai.acp.custom").to_string()
            } else {
                preset.name.into()
            };
            actions = actions.child(
                NyaButton::new(format!("acp-add-{strategy}"), format!("+ {label}"))
                    .form_control()
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
        let expand_focus = self
            .agent_disclosure_focus
            .entry((id.clone(), "expanded"))
            .or_insert_with(|| cx.focus_handle())
            .clone();
        let advanced_focus = self
            .agent_disclosure_focus
            .entry((id.clone(), "advanced"))
            .or_insert_with(|| cx.focus_handle())
            .clone();
        let expanded = editor.expanded.contains(&id);
        let advanced = editor.advanced.contains(&id);
        let help = editor.installation_help.contains(&id);
        let presentation = project(profile, &editor, status);
        let unavailable = execution_unavailable_key(profile);
        let enable_id = id.clone();
        let expand_id = id.clone();
        let copy_id = id.clone();
        let delete_id = id.clone();
        let copy_panel = cx.entity().downgrade();
        let delete_panel = cx.entity().downgrade();
        let strategy = profile
            .strategy
            .preset()
            .map(|preset| preset.name)
            .unwrap_or(&profile.strategy.0);
        let more = NyaDropdownMenu::new(format!("acp-more-{id}"))
            .icon("icons/conn/more.svg")
            .tooltip(t!("ai.acp.more"))
            .items([
                NyaMenuItem::action(t!("ai.acp.copy")).on_click(move |_, _, cx| {
                    let _ = copy_panel.update(cx, |panel, cx| {
                        panel.with_app(cx, |app, cx| {
                            app.edit_acp_profile(&copy_id, "copy", String::new(), cx)
                        });
                    });
                }),
                NyaMenuItem::separator(),
                NyaMenuItem::action(t!("ai.acp.delete"))
                    .danger()
                    .on_click(move |_, _, cx| {
                        let _ = delete_panel.update(cx, |panel, cx| {
                            panel.with_app(cx, |app, cx| {
                                app.edit_acp_profile(
                                    &delete_id,
                                    "request-delete",
                                    String::new(),
                                    cx,
                                )
                            });
                        });
                    }),
            ]);
        let mut summary = div()
            .min_w(px(160.))
            .flex_1()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .text_size(px(14.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(profile.name.clone()),
            )
            .when(profile.name != strategy, |this| {
                this.child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(palette.text_muted))
                        .child(strategy.to_owned()),
                )
            })
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(if presentation.warning {
                        palette.warning
                    } else {
                        palette.text_muted
                    }))
                    .child(t!(presentation.message_key)),
            );
        if editor.dirty.contains(&id) {
            summary = summary.child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(palette.text_muted))
                    .child(t!("settings.unappliedChanges")),
            );
        }
        let mut card = div()
            .min_w_0()
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
                    .gap_3()
                    .child(summary)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_size(px(12.))
                            .child(t!("ai.acp.enable"))
                            .child(
                                div()
                                    .debug_selector({
                                        let id = id.clone();
                                        move || format!("acp-enabled-{id}")
                                    })
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
                            )
                            .child(
                                div()
                                    .debug_selector({
                                        let id = id.clone();
                                        move || format!("acp-expand-{id}")
                                    })
                                    .child(
                                        NyaButton::new(
                                            format!("acp-expand-{id}"),
                                            t!(if expanded {
                                                "ai.acp.collapse"
                                            } else {
                                                "ai.acp.expand"
                                            }),
                                        )
                                        .form_control()
                                        .track_focus(&expand_focus)
                                        .variant(NyaButtonVariant::Ghost)
                                        .icon(if expanded {
                                            "icons/chevron-up.svg"
                                        } else {
                                            "icons/chevron-down.svg"
                                        })
                                        .on_click(
                                            cx.listener(move |panel, _, window, cx| {
                                                expand_focus.focus(window, cx);
                                                panel.with_app(cx, |app, cx| {
                                                    app.edit_acp_profile(
                                                        &expand_id,
                                                        "expanded",
                                                        String::new(),
                                                        cx,
                                                    )
                                                });
                                            }),
                                        ),
                                    ),
                            )
                            .child(more),
                    ),
            );
        if profile.strategy.manages_local_tools() {
            card = card.child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(palette.text_muted))
                    .child(t!("ai.acp.localToolsHint")),
            );
        }
        if !expanded {
            return card;
        }
        let columns = if self.content_width >= 640. { 2 } else { 1 };
        let choose_id = id.clone();
        card = card
            .child(
                div()
                    .grid()
                    .grid_cols(columns)
                    .gap_3()
                    .child(self.ai_agent_field(&id, "name", "ai.acp.name", None))
                    .child(self.ai_agent_field(&id, "model", "ai.acp.model", None)),
            )
            .child(
                div()
                    .min_w_0()
                    .debug_selector({
                        let id = id.clone();
                        move || format!("acp-profile-field-{id}-executable")
                    })
                    .child(ai_field(
                        palette,
                        t!("ai.acp.executable"),
                        Some(t!("ai.acp.executableHint").into()),
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().flex_1().min_w_0().child(self.existing_text_input_box(
                                format!("ai.agent.{id}.executable"),
                                false,
                            )))
                            .child(
                                NyaButton::new(
                                    format!("acp-choose-{id}"),
                                    t!("ai.acp.chooseExecutable"),
                                )
                                .form_control()
                                .on_click(cx.listener(
                                    move |panel, _, _, cx| {
                                        panel.with_app(cx, |app, cx| {
                                            app.choose_acp_executable(choose_id.clone(), cx)
                                        });
                                    },
                                )),
                            ),
                    )),
            );
        let install = presentation.action == ProfileAction::Install;
        let inspect_id = id.clone();
        let help_id = id.clone();
        let mut actions = div().flex().flex_wrap().gap_2();
        if unavailable.is_none() && profile.strategy.supported() {
            actions = actions.child(
                NyaButton::new(format!("acp-help-{id}"), t!("ai.acp.installHelp"))
                    .form_control()
                    .variant(if install {
                        NyaButtonVariant::Primary
                    } else {
                        NyaButtonVariant::Ghost
                    })
                    .on_click(cx.listener(move |panel, _, _, cx| {
                        panel.with_app(cx, |app, cx| {
                            app.edit_acp_profile(&help_id, "installation-help", String::new(), cx)
                        });
                    })),
            );
        }
        actions = actions.child(
            NyaButton::new(
                format!("acp-inspect-{id}"),
                t!(if presentation.busy {
                    "ai.acp.checking"
                } else if presentation.action == ProfileAction::Check {
                    "ai.acp.check"
                } else {
                    "ai.acp.recheck"
                }),
            )
            .form_control()
            .variant(if presentation.action == ProfileAction::Check {
                NyaButtonVariant::Primary
            } else {
                NyaButtonVariant::Secondary
            })
            .loading(presentation.busy)
            .disabled(status.pending || presentation.action == ProfileAction::Unavailable)
            .on_click(cx.listener(move |panel, _, _, cx| {
                panel.with_app(cx, |app, cx| app.inspect_acp_profile(&inspect_id, cx));
            })),
        );
        if presentation.busy {
            actions = actions.child(
                NyaButton::new(format!("acp-cancel-{id}"), t!("ai.acp.cancel"))
                    .form_control()
                    .on_click(cx.listener(|panel, _, _, cx| {
                        panel.with_app(cx, |app, cx| {
                            app.ai.cancel_agent_management();
                            app.request_settings_panel_refresh(cx);
                        });
                    })),
            );
        }
        card = card.child(actions);
        if help && unavailable.is_none() {
            use nyaterm_agent_client::acp::policy;
            let preset = profile.strategy.preset().expect("available strategy");
            let url = preset.documentation;
            let mut installation = div()
                .flex()
                .flex_col()
                .gap_2()
                .p_3()
                .rounded_md()
                .bg(rgb(palette.bg))
                .text_size(px(12.));
            if profile.strategy.manages_local_tools() {
                installation = installation.child(t!("ai.acp.genericInstall"));
            } else {
                let (version, engine) = if profile.strategy.0 == "codex" {
                    (policy::CODEX_ADAPTER_VERSION, policy::CODEX_ENGINE_VERSION)
                } else {
                    (
                        policy::CLAUDE_ADAPTER_VERSION,
                        policy::CLAUDE_ENGINE_VERSION,
                    )
                };
                installation = installation
                    .child(t!("ai.acp.installExplanation"))
                    .child(t!("ai.acp.adapterVersion", version = version))
                    .child(t!(
                        "ai.acp.engineVersion",
                        agent = strategy,
                        version = engine
                    ));
            }
            card = card.child(
                installation.child(
                    NyaButton::new(format!("acp-install-docs-{id}"), t!("ai.acp.installDocs"))
                        .form_control()
                        .on_click(move |_, _, cx| cx.open_url(url)),
                ),
            );
        }
        if let Some(state) = status.profiles.get(&id)
            && state.problem.is_none()
            && unavailable.is_none()
            && !presentation.busy
        {
            let mut account = div().flex().flex_wrap().gap_2();
            for (method, label) in &state.auth_methods {
                let (id, method) = (id.clone(), method.clone());
                account = account.child(
                    NyaButton::new(format!("acp-login-{id}-{method}"), label.clone())
                        .form_control()
                        .variant(if presentation.action == ProfileAction::Login {
                            NyaButtonVariant::Primary
                        } else {
                            NyaButtonVariant::Secondary
                        })
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
                        .form_control()
                        .disabled(status.pending)
                        .on_click(cx.listener(move |panel, _, _, cx| {
                            panel.with_app(cx, |app, cx| app.acp_account_action(&id, None, cx));
                        })),
                );
            }
            if !state.auth_methods.is_empty() || state.logout {
                card = card.child(account).child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(palette.text_muted))
                        .child(t!("ai.acp.accountImmediate")),
                );
            } else if state.authenticated != Some(true) {
                card = card.child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(palette.text_muted))
                        .child(t!("ai.acp.cliLoginHint")),
                );
            }
        }
        let advanced_id = id.clone();
        card = card.child(
            div()
                .pt_3()
                .border_t_1()
                .border_color(rgb(palette.border))
                .child(
                    NyaButton::new(
                        format!("acp-advanced-{id}"),
                        t!(if advanced {
                            "ai.acp.hideAdvanced"
                        } else {
                            "ai.acp.advanced"
                        }),
                    )
                    .form_control()
                    .track_focus(&advanced_focus)
                    .label_left()
                    .variant(NyaButtonVariant::Ghost)
                    .icon(if advanced {
                        "icons/chevron-up.svg"
                    } else {
                        "icons/chevron-down.svg"
                    })
                    .on_click(cx.listener(move |panel, _, window, cx| {
                        advanced_focus.focus(window, cx);
                        panel.with_app(cx, |app, cx| {
                            app.edit_acp_profile(&advanced_id, "advanced", String::new(), cx)
                        });
                    })),
                ),
        );
        if advanced {
            card = card.child(self.ai_agent_advanced_fields(profile, columns, cx));
        }
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
                            .form_control()
                            .variant(NyaButtonVariant::Danger)
                            .on_click(cx.listener(move |panel, _, _, cx| {
                                panel.with_app(cx, |app, cx| {
                                    app.edit_acp_profile(&remove_id, "delete", String::new(), cx)
                                });
                            })),
                    )
                    .child(
                        NyaButton::new(format!("acp-cancel-delete-{id}"), t!("common.cancel"))
                            .form_control()
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
        let mut strategies: Vec<_> = AGENT_PRESETS
            .iter()
            .map(|preset| {
                NyaSelectOption::new(
                    preset.strategy,
                    if preset.strategy == "custom_acp" {
                        t!("ai.acp.custom").to_string()
                    } else {
                        preset.name.into()
                    },
                )
            })
            .collect();
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
            if field == "config"
                && profile.strategy.manages_local_tools()
                && profile.config_directory.is_none()
            {
                continue;
            }
            let hint = if field == "args" && profile.strategy.manages_local_tools() {
                Some("ai.acp.argsHint")
            } else {
                hint
            };
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
                            .form_control()
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
                .form_control()
                .on_click(cx.listener(move |panel, _, _, cx| {
                    panel.with_app(cx, |app, cx| {
                        app.edit_acp_profile(&add_id, "add-env", String::new(), cx)
                    });
                })),
        )
    }
}

#[cfg(test)]
mod tests;
