use super::super::settings_switch;
use super::{ai_card, ai_provider_row};
use crate::features::ai::ConnectionStatus;
use crate::features::pages::settings::panel::SettingsPanel;
use crate::theme::ThemePalette;
use gpui::{
    AnyElement, Context, FontWeight, IntoElement, KeyDownEvent, SharedString, div, img, prelude::*,
    px, rgb, svg,
};
use nyaterm_core::MAX_AI_CUSTOM_HEADERS;
use nyaterm_core::ai::provider_settings::{
    CustomHeaderError, DEFAULT_MODEL_REASONING_EFFORTS, MODEL_REASONING_EFFORTS,
    prioritized_provider_models, protocol_value, provider_label, provider_name_taken,
    provider_requires_api_key,
};
use nyaterm_core::ai::{AiModelSource, AiProviderKind};
use nyaterm_ui::{NyaButton, NyaButtonVariant, NyaIconButton, NyaSelectOption};
use rust_i18n::t;

impl SettingsPanel {
    pub(in crate::features) fn ai_models_settings_section(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let config = self.ai.settings_config().clone();
        let view = self.ai.providers.clone();
        let credentials: Vec<_> = config
            .provider_credentials
            .iter()
            .filter(|credential| credential.enabled)
            .cloned()
            .collect();
        let selected = credentials
            .iter()
            .find(|credential| Some(&credential.id) == view.selected_id.as_ref())
            .cloned();
        let testing = view
            .statuses
            .values()
            .any(|status| *status == ConnectionStatus::Testing);
        let actions = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(
                NyaIconButton::new("ai-provider-refresh", "icons/fe/refresh.svg")
                    .tooltip(t!("ai.refreshProviders"))
                    .disabled(testing || credentials.is_empty())
                    .on_click(cx.listener(|panel, _, _, cx| {
                        panel.with_app(cx, |app, cx| app.test_ai_provider(true, true, cx));
                    })),
            )
            .child(
                NyaButton::new("ai-provider-add", t!("ai.addProvider"))
                    .icon("icons/settings/plus.svg")
                    .selected(view.choices_open)
                    .on_click(cx.listener(|panel, _, _, cx| {
                        panel.with_app(cx, |app, cx| app.toggle_ai_provider_choices(cx));
                    })),
            );
        let columns = if self.viewport_width >= 800. {
            3
        } else if self.viewport_width >= 520. {
            2
        } else {
            1
        };
        let mut providers = div()
            .id("ai-provider-list")
            .w_full()
            .min_w_0()
            .grid()
            .grid_cols(columns)
            .gap_2();
        for credential in &credentials {
            let id = credential.id.clone();
            let is_selected = view.selected_id.as_ref() == Some(&id);
            let status = view.statuses.get(&id).copied().unwrap_or_default();
            let status_label = match status {
                ConnectionStatus::Idle => t!("ai.providerStatusIdle"),
                ConnectionStatus::Testing => t!("ai.providerStatusTesting"),
                ConnectionStatus::Success => t!("ai.providerStatusSuccess"),
                ConnectionStatus::Error => t!("ai.providerStatusError"),
            };
            let name = if credential.name.trim().is_empty() {
                provider_label(&credential.provider_kind).to_string()
            } else {
                credential.name.clone()
            };
            providers = providers.child(
                div()
                    .min_w_0()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(if is_selected {
                        palette.accent
                    } else {
                        palette.border
                    }))
                    .bg(rgb(palette.surface_elevated))
                    .overflow_hidden()
                    .child(
                        NyaButton::new(format!("ai-provider-{id}"), "")
                            .variant(NyaButtonVariant::Ghost)
                            .height(px(64.))
                            .full_width()
                            .selected(is_selected)
                            .tooltip(name.clone())
                            .content(
                                div()
                                    .w_full()
                                    .min_w_0()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .flex_none()
                                            .child(self.provider_badge(credential, palette)),
                                    )
                                    .child(
                                        div()
                                            .min_w_0()
                                            .flex_1()
                                            .flex()
                                            .flex_col()
                                            .gap_1()
                                            .child(
                                                div()
                                                    .w_full()
                                                    .truncate()
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .child(name),
                                            )
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap_1()
                                                    .text_size(px(11.))
                                                    .text_color(rgb(status_color(palette, status)))
                                                    .child(status_dot(palette, status))
                                                    .child(
                                                        div()
                                                            .min_w_0()
                                                            .truncate()
                                                            .child(status_label),
                                                    ),
                                            ),
                                    )
                                    .child(div().w(px(14.)).flex_none().when(
                                        is_selected,
                                        |mark| {
                                            mark.child(
                                                svg()
                                                    .path("icons/check.svg")
                                                    .size(px(14.))
                                                    .text_color(rgb(palette.accent)),
                                            )
                                        },
                                    )),
                            )
                            .on_click(cx.listener(move |panel, _, _, cx| {
                                panel
                                    .with_app(cx, |app, cx| app.select_ai_provider(id.clone(), cx));
                            })),
                    ),
            );
        }
        let mut picker = div().flex().flex_wrap().gap_2();
        if view.choices_open || credentials.is_empty() {
            for kind in [
                AiProviderKind::Openai,
                AiProviderKind::Anthropic,
                AiProviderKind::Gemini,
                AiProviderKind::Deepseek,
                AiProviderKind::Groq,
                AiProviderKind::Ollama,
                AiProviderKind::Xai,
                AiProviderKind::Cohere,
                AiProviderKind::Mimo,
                AiProviderKind::Zai,
                AiProviderKind::OpenaiCompatible,
            ] {
                let label = if kind == AiProviderKind::OpenaiCompatible {
                    t!("ai.customProviderOption").to_string()
                } else {
                    provider_label(&kind).to_string()
                };
                picker = picker.child(
                    NyaButton::new(format!("ai-provider-preset-{kind:?}"), label).on_click(
                        cx.listener(move |panel, _, _, cx| {
                            panel.with_app(cx, |app, cx| {
                                app.add_ai_provider_preset(kind.clone(), cx)
                            });
                        }),
                    ),
                );
            }
        }
        let mut page = div().flex().flex_col().gap_5().child(ai_card(
            palette,
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .child(t!("ai.providerList"))
                .child(
                    div()
                        .text_size(px(12.))
                        .font_weight(FontWeight::NORMAL)
                        .text_color(rgb(palette.text_muted))
                        .child(t!("ai.providerCount", count = credentials.len())),
                ),
            actions,
            div()
                .flex()
                .flex_col()
                .gap_3()
                .when(!credentials.is_empty(), |body| body.child(providers))
                .when(view.choices_open || credentials.is_empty(), |body| {
                    body.child(picker)
                }),
        ));
        let Some(credential) = selected else {
            return page;
        };
        let id = credential.id.clone();
        let title = div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                NyaButton::new("ai-provider-icon", "")
                    .variant(NyaButtonVariant::Ghost)
                    .compact()
                    .content(self.provider_badge(&credential, palette))
                    .disabled(credential.provider_kind != AiProviderKind::OpenaiCompatible)
                    .tooltip(t!("ai.changeProviderIcon"))
                    .on_click(cx.listener({
                        let id = id.clone();
                        move |panel, _, _, cx| {
                            panel.with_app(cx, |app, cx| {
                                app.prompt_ai_provider_icon(id.clone(), cx)
                            });
                        }
                    })),
            )
            .child(credential.name.clone());
        let delete = NyaIconButton::new("ai-provider-delete", "icons/settings/trash.svg")
            .tooltip(t!("ai.deleteProvider"))
            .on_click(cx.listener({
                let id = id.clone();
                move |panel, _, window, cx| {
                    panel.with_app(cx, |app, cx| {
                        app.remove_ai_provider_confirm(id.clone(), window, cx)
                    });
                }
            }));
        let duplicate = provider_name_taken(&credential.name, &config.provider_credentials, &id);
        let name = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(self.existing_text_input_box(format!("ai.credential.{id}.name"), false))
            .when(duplicate, |body| {
                body.child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(palette.danger))
                        .child(t!("ai.providerNameDuplicate")),
                )
            });
        let missing_key = provider_requires_api_key(&credential)
            && credential
                .api_key
                .as_deref()
                .is_none_or(|key| key.is_empty())
            && !self.ai.credential_draft_key_ids.contains(&id);
        let url = self.existing_text_input_box(format!("ai.credential.{id}.base-url"), false);
        let mut protocols = vec![
            NyaSelectOption::new("chat_completions", t!("ai.providerProtocolChatCompletions")),
            NyaSelectOption::new("responses", t!("ai.providerProtocolResponses")),
            NyaSelectOption::new("anthropic", t!("ai.providerProtocolAnthropic")),
        ];
        protocols.push(NyaSelectOption::new(
            "gemini",
            t!("ai.providerProtocolGemini"),
        ));
        protocols.push(NyaSelectOption::new(
            "ollama",
            t!("ai.providerProtocolOllama"),
        ));
        let protocol = self.form_select_control(
            format!("ai-provider-protocol.{id}"),
            protocols,
            Some(protocol_value(&credential).into()),
            false,
            cx,
        );
        let key_id = format!("ai.credential.{id}.api-key");
        let key = div()
            .w_full()
            .children(self.existing_text_input(&key_id).map(|field| {
                nyaterm_ui::NyaInputShell::new(key_id, &field).on_mask_toggle(cx.listener({
                    let id = id.clone();
                    move |panel, _, _, cx| {
                        panel.with_app(cx, |app, cx| app.toggle_ai_provider_secret(id.clone(), cx));
                    }
                }))
            }));
        let status = view.statuses.get(&id).copied().unwrap_or_default();
        let test = div()
            .flex()
            .items_center()
            .justify_end()
            .gap_2()
            .child(status_dot(palette, status))
            .child(
                NyaButton::new("ai-provider-test", t!("ai.connectionTest"))
                    .disabled(
                        testing
                            || duplicate
                            || missing_key
                            || credential
                                .base_url
                                .as_deref()
                                .is_none_or(|url| url.trim().is_empty()),
                    )
                    .on_click(cx.listener(|panel, _, _, cx| {
                        panel.with_app(cx, |app, cx| app.test_ai_provider(false, false, cx));
                    })),
            );
        let mut header_name_counts = std::collections::HashMap::new();
        for header in &credential.extra_headers {
            let name = header.name.trim().to_lowercase();
            if !name.is_empty() {
                *header_name_counts.entry(name).or_insert(0_usize) += 1;
            }
        }
        let mut custom_headers = div().flex().flex_col().gap_2().child(
            div()
                .text_size(px(12.))
                .text_color(rgb(palette.text_muted))
                .child(t!("ai.customHeaderDesc")),
        );
        for header in &credential.extra_headers {
            let header_id = header.id.clone();
            let name_id = format!("ai.credential.{id}.header.{header_id}.name");
            let value_id = format!("ai.credential.{id}.header.{header_id}.value");
            let header_error = self.ai.header_errors.get(&(id.clone(), header_id.clone()));
            let name_duplicate = header_name_counts
                .get(&header.name.trim().to_lowercase())
                .is_some_and(|count| *count > 1);
            let value_field = div()
                .w_full()
                .children(self.existing_text_input(&value_id).map(|field| {
                    let credential_id = id.clone();
                    let header_id = header_id.clone();
                    nyaterm_ui::NyaInputShell::new(value_id, &field).on_mask_toggle(cx.listener(
                        move |panel, _, _, cx| {
                            panel.with_app(cx, |app, cx| {
                                app.toggle_ai_provider_header_secret(
                                    credential_id.clone(),
                                    header_id.clone(),
                                    cx,
                                )
                            });
                        },
                    ))
                }));
            custom_headers = custom_headers.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .child(self.existing_text_input_box(name_id, false)),
                            )
                            .child(div().min_w_0().flex_1().child(value_field))
                            .child(
                                NyaIconButton::new(
                                    format!("ai-provider-header-delete-{header_id}"),
                                    "icons/settings/trash.svg",
                                )
                                .tooltip(t!("common.delete"))
                                .on_click(cx.listener({
                                    let id = id.clone();
                                    let header_id = header_id.clone();
                                    move |panel, _, _, cx| {
                                        panel.with_app(cx, |app, cx| {
                                            app.remove_ai_provider_header(
                                                id.clone(),
                                                header_id.clone(),
                                                cx,
                                            )
                                        });
                                    }
                                })),
                            ),
                    )
                    .when_some(
                        header_error.filter(|error| **error != CustomHeaderError::DuplicateName),
                        |body, error| {
                            body.child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(palette.danger))
                                    .child(t!(match error {
                                        CustomHeaderError::InvalidName =>
                                            "ai.customHeaderNameInvalid",
                                        CustomHeaderError::ManagedName => "ai.customHeaderManaged",
                                        CustomHeaderError::EmptyValue =>
                                            "ai.customHeaderValueRequired",
                                        CustomHeaderError::ValueTooLong =>
                                            "ai.customHeaderValueTooLong",
                                        CustomHeaderError::InvalidValue =>
                                            "ai.customHeaderValueInvalid",
                                        CustomHeaderError::DuplicateName =>
                                            "ai.customHeaderNameDuplicate",
                                        CustomHeaderError::TooMany => "ai.customHeaderTooMany",
                                    })),
                            )
                        },
                    )
                    .when(name_duplicate, |body| {
                        body.child(
                            div()
                                .text_size(px(12.))
                                .text_color(rgb(palette.danger))
                                .child(t!("ai.customHeaderNameDuplicate")),
                        )
                    }),
            );
        }
        custom_headers = custom_headers.child(
            NyaButton::new("ai-provider-header-add", t!("ai.customHeaderAdd"))
                .icon("icons/settings/plus.svg")
                .disabled(credential.extra_headers.len() >= MAX_AI_CUSTOM_HEADERS)
                .on_click(cx.listener({
                    let id = id.clone();
                    move |panel, _, _, cx| {
                        panel.with_app(cx, |app, cx| app.add_ai_provider_header(id.clone(), cx));
                    }
                })),
        );
        let fields = div()
            .flex()
            .flex_col()
            .child(ai_provider_row(palette, t!("ai.profileName"), name))
            .child(ai_provider_row(
                palette,
                format!("{} *", t!("ai.providerEndpoint")),
                url,
            ))
            .child(ai_provider_row(
                palette,
                t!("ai.providerProtocol"),
                protocol,
            ))
            .child(ai_provider_row(
                palette,
                format!(
                    "{}{}",
                    t!("settings.apiKey"),
                    if provider_requires_api_key(&credential) {
                        " *"
                    } else {
                        ""
                    }
                ),
                key,
            ))
            .child(ai_provider_row(
                palette,
                t!("ai.customHeaderTitle"),
                custom_headers,
            ))
            .child(div().py_3().child(test));
        page = page.child(ai_card(palette, title, delete, fields));
        let refresh = NyaIconButton::new("ai-provider-model-refresh", "icons/fe/refresh.svg")
            .tooltip(t!("ai.refreshModels"))
            .disabled(testing)
            .on_click(cx.listener(|panel, _, _, cx| {
                panel.with_app(cx, |app, cx| app.test_ai_provider(false, true, cx));
            }));
        let mut rows = div().flex().flex_col();
        let draft = self
            .ai
            .settings_manual_model_drafts()
            .get(&id)
            .cloned()
            .unwrap_or_default();
        rows = rows.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .pb_3()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(self.existing_text_input_box(
                            format!("ai.settings.manual-model.{id}"),
                            false,
                        ))
                        .on_key_down(cx.listener({
                            let id = id.clone();
                            move |panel, event: &KeyDownEvent, window, cx| {
                                if panel.handle_ai_manual_model_key_down(&id, event, window, cx) {
                                    cx.stop_propagation();
                                }
                            }
                        })),
                )
                .child(
                    NyaButton::new("ai-provider-model-add", t!("common.add"))
                        .form_control()
                        .icon("icons/settings/plus.svg")
                        .disabled(draft.trim().is_empty())
                        .on_click(cx.listener({
                            let id = id.clone();
                            move |panel, _, _, cx| {
                                let name = panel
                                    .ai
                                    .settings_manual_model_drafts()
                                    .get(&id)
                                    .cloned()
                                    .unwrap_or_default();
                                panel.with_app(cx, |app, cx| {
                                    app.add_ai_manual_model(id.clone(), name, cx);
                                    app.clear_ai_manual_model_draft(&id, cx);
                                    app.ai.refresh_provider_model_order();
                                });
                            }
                        })),
                ),
        );
        let models = prioritized_provider_models(&config.models, &view.model_order, &draft);
        let draft_name = draft.trim();
        if let Some(existing) = models
            .iter()
            .find(|model| !draft_name.is_empty() && model.name == draft_name)
        {
            rows = rows.child(
                div()
                    .pb_3()
                    .text_size(px(12.))
                    .text_color(rgb(palette.text_muted))
                    .child(t!("ai.manualModelExists", model = existing.name.clone())),
            );
        }
        for model in models {
            let model_id = model.id.clone();
            let model_status = view
                .model_statuses
                .get(&model_id)
                .copied()
                .unwrap_or_default();
            let mut label = div()
                .min_w_0()
                .flex_1()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .text_size(px(12.))
                        .child(model.name.clone()),
                );
            if model.last_seen_at.is_some() {
                label = label.child(model_badge(palette, t!("ai.providerReturnedBadge")));
            }
            if model.source == AiModelSource::Manual {
                label = label.child(model_badge(palette, t!("ai.manualModelBadge")));
            }
            let row = div()
                .py_2()
                .flex()
                .items_center()
                .gap_2()
                .child(label)
                .child(status_dot(palette, model_status))
                .child(
                    NyaIconButton::new(
                        format!("ai-model-test-{model_id}"),
                        "icons/settings/link.svg",
                    )
                    .tooltip(t!("ai.testModel"))
                    .disabled(
                        view.model_statuses
                            .values()
                            .any(|status| *status == ConnectionStatus::Testing),
                    )
                    .on_click(cx.listener({
                        let id = model_id.clone();
                        move |panel, _, _, cx| {
                            panel.with_app(cx, |app, cx| app.test_ai_model(id.clone(), cx));
                        }
                    })),
                )
                .child(
                    NyaIconButton::new(
                        format!("ai-model-edit-{model_id}"),
                        "icons/settings/edit.svg",
                    )
                    .tooltip(t!("ai.editModelConfig"))
                    .on_click(cx.listener({
                        let id = model_id.clone();
                        move |panel, _, _, cx| {
                            panel.with_app(cx, |app, cx| app.edit_ai_model(id.clone(), cx));
                        }
                    })),
                )
                .child(
                    NyaIconButton::new(
                        format!("ai-model-delete-{model_id}"),
                        "icons/settings/trash.svg",
                    )
                    .tooltip(t!("ai.deleteModel"))
                    .on_click(cx.listener({
                        let id = model_id.clone();
                        move |panel, _, _, cx| {
                            panel.with_app(cx, |app, cx| app.remove_ai_model(id.clone(), cx));
                        }
                    })),
                )
                .child(settings_switch(
                    palette,
                    format!("ai-model-toggle-{model_id}"),
                    model.enabled,
                    cx.listener({
                        let id = model_id.clone();
                        move |panel, _, _, cx| {
                            panel.toggle_ai_model_enabled(id.clone(), cx);
                        }
                    }),
                ));
            let mut item = div()
                .border_b_1()
                .border_color(rgb(palette.border))
                .child(row);
            if view.editing_model.as_ref() == Some(&model_id) {
                let supported = model
                    .supported_reasoning_efforts
                    .as_deref()
                    .unwrap_or(&DEFAULT_MODEL_REASONING_EFFORTS);
                let mut choices = div().flex().flex_wrap().gap_2();
                for effort in MODEL_REASONING_EFFORTS {
                    let value = serde_json::to_value(effort)
                        .ok()
                        .and_then(|value| value.as_str().map(str::to_string))
                        .unwrap_or_default();
                    choices = choices.child(
                        NyaButton::new(format!("ai-model-effort-{model_id}-{value}"), value)
                            .small()
                            .selected(supported.contains(&effort))
                            .on_click(cx.listener({
                                let id = model_id.clone();
                                move |panel, _, _, cx| {
                                    panel.with_app(cx, |app, cx| {
                                        app.toggle_ai_model_reasoning(id.clone(), effort, cx)
                                    });
                                }
                            })),
                    );
                }
                item = item.child(
                    div()
                        .py_3()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(t!("ai.supportedReasoningEfforts"))
                        .child(choices),
                );
            }
            rows = rows.child(item);
        }
        if view.model_order.is_empty() {
            rows = rows.child(
                div()
                    .py_5()
                    .text_size(px(12.))
                    .text_color(rgb(palette.text_muted))
                    .child(t!("ai.noModels")),
            );
        }
        if !config.models.iter().any(|model| model.enabled) {
            rows = rows.child(
                div()
                    .pt_3()
                    .text_size(px(12.))
                    .text_color(rgb(palette.warning))
                    .child(t!("ai.enableOneModelHint")),
            );
        }
        page = page.child(ai_card(
            palette,
            div().child(t!("ai.modelList")),
            refresh,
            rows,
        ));
        if let Some(message) = view.message {
            page = page.child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(palette.warning))
                    .child(message),
            );
        }
        page
    }

    fn provider_badge(
        &self,
        credential: &nyaterm_core::AiProviderCredential,
        palette: ThemePalette,
    ) -> AnyElement {
        if credential.provider_kind == AiProviderKind::Cohere {
            return div()
                .size(px(24.))
                .rounded_full()
                .bg(rgb(palette.surface_elevated))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(10.))
                .text_color(rgb(palette.accent))
                .child("C")
                .into_any_element();
        }
        if let Some(image) = self.ai.providers.icons.get(&credential.id) {
            return img(image.clone())
                .size(px(24.))
                .rounded_full()
                .into_any_element();
        }
        let path = match credential.provider_kind {
            AiProviderKind::Openai => "icons/settings/openai.svg",
            AiProviderKind::Anthropic => "icons/settings/anthropic.svg",
            AiProviderKind::Gemini => "icons/settings/gemini.svg",
            AiProviderKind::Deepseek => "icons/settings/deepseek.svg",
            AiProviderKind::Xai => "icons/settings/xai.svg",
            AiProviderKind::Zai => "icons/settings/zai.svg",
            AiProviderKind::Ollama => "icons/settings/ollama.svg",
            AiProviderKind::Mimo => "icons/settings/mimo.svg",
            _ => "icons/settings/cloud.svg",
        };
        div()
            .size(px(24.))
            .rounded_full()
            .bg(rgb(palette.surface_elevated))
            .flex()
            .items_center()
            .justify_center()
            .child(
                svg()
                    .path(path)
                    .size(px(16.))
                    .text_color(rgb(palette.accent)),
            )
            .into_any_element()
    }
}

fn status_dot(palette: ThemePalette, status: ConnectionStatus) -> impl IntoElement {
    div()
        .size(px(9.))
        .rounded_full()
        .flex_none()
        .bg(rgb(status_color(palette, status)))
}

fn status_color(palette: ThemePalette, status: ConnectionStatus) -> u32 {
    match status {
        ConnectionStatus::Idle => palette.text_muted,
        ConnectionStatus::Testing => palette.accent,
        ConnectionStatus::Success => palette.success,
        ConnectionStatus::Error => palette.danger,
    }
}
fn model_badge(palette: ThemePalette, text: impl Into<SharedString>) -> impl IntoElement {
    div()
        .flex_none()
        .rounded_sm()
        .border_1()
        .border_color(rgb(palette.border))
        .px_1()
        .text_size(px(10.))
        .text_color(rgb(palette.text_muted))
        .child(text.into())
}
