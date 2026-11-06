use super::super::{settings_form_row, settings_switch};
use super::{ai_card, ai_field};
use crate::features::pages::settings::panel::SettingsPanel;
use gpui::{Context, IntoElement, div, prelude::*, px, rgb, svg};
use nyaterm_ui::{NyaSelectOption, NyaTooltip};
use rust_i18n::t;

impl SettingsPanel {
    pub(super) fn ai_general_content(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = self.theme_palette();
        let settings = self.ai.settings_config().clone();
        let switches = div()
            .flex()
            .flex_col()
            .gap_4()
            .child(settings_form_row(
                palette,
                t!("ai.enabled"),
                None,
                settings_switch(
                    palette,
                    "ai-enabled",
                    settings.enabled,
                    cx.listener(|panel, _, _, cx| panel.toggle_ai_enabled(cx)),
                ),
            ))
            .child(settings_form_row(
                palette,
                t!("ai.redaction"),
                None,
                settings_switch(
                    palette,
                    "ai-redaction-toggle",
                    settings.redaction_enabled,
                    cx.listener(|panel, _, _, cx| panel.toggle_ai_redaction(cx)),
                ),
            ))
            .child(settings_form_row(
                palette,
                t!("ai.allowSave"),
                None,
                settings_switch(
                    palette,
                    "ai-save-command-toggle",
                    settings.allow_save_command,
                    cx.listener(|panel, _, _, cx| panel.toggle_ai_allow_save_command(cx)),
                ),
            ))
            .child(settings_form_row(
                palette,
                t!("ai.recordHistory"),
                None,
                settings_switch(
                    palette,
                    "ai-history-toggle",
                    settings.record_history,
                    cx.listener(|panel, _, _, cx| panel.toggle_ai_record_history(cx)),
                ),
            ));
        let fields = div()
            .grid()
            .grid_cols(if self.content_width >= 1024. { 2 } else { 1 })
            .gap_4()
            .child(
                ai_field(
                    palette,
                    t!("ai.contextLineLimit"),
                    None,
                    self.existing_number_input_box("ai.number.context-line-limit"),
                )
                .debug_selector(|| "ai-general-context-field".into()),
            )
            .child(
                ai_field(
                    palette,
                    t!("ai.timeoutMs"),
                    None,
                    self.existing_number_input_box("ai.number.timeout-ms"),
                )
                .debug_selector(|| "ai-general-timeout-field".into()),
            );
        use nyaterm_core::ai::proxy::{AiProxyMode, AiProxyProtocol};
        let proxy_mode = self.form_select_control(
            "ai-proxy-mode",
            [
                ("system", "ai.proxySystem"),
                ("direct", "ai.proxyDirect"),
                ("custom", "ai.proxyCustom"),
            ]
            .into_iter()
            .map(|(value, label)| NyaSelectOption::new(value, t!(label)))
            .collect(),
            Some(
                match settings.proxy.mode {
                    AiProxyMode::System => "system",
                    AiProxyMode::Direct => "direct",
                    AiProxyMode::Custom => "custom",
                }
                .into(),
            ),
            false,
            cx,
        );
        let proxy_protocol = self.form_select_control(
            "ai-proxy-protocol",
            vec![
                NyaSelectOption::new("http", "HTTP"),
                NyaSelectOption::new("socks5", "SOCKS5"),
            ],
            Some(
                if settings.proxy.protocol == AiProxyProtocol::Http {
                    "http"
                } else {
                    "socks5"
                }
                .into(),
            ),
            false,
            cx,
        );
        let proxy_fields = div()
            .flex()
            .flex_col()
            .gap_4()
            .child(ai_field(
                palette,
                t!("ai.proxyMode"),
                Some(t!("ai.proxyDescription").into()),
                proxy_mode,
            ))
            .when(settings.proxy.mode == AiProxyMode::Custom, |fields| {
                fields
                    .child(ai_field(
                        palette,
                        t!("ai.proxyProtocol"),
                        None,
                        proxy_protocol,
                    ))
                    .child(ai_field(
                        palette,
                        t!("ai.proxyHost"),
                        None,
                        self.existing_text_input_box("ai.input.proxy-host", false),
                    ))
                    .child(ai_field(
                        palette,
                        t!("ai.proxyPort"),
                        None,
                        self.existing_text_input_box("ai.input.proxy-port", false),
                    ))
                    .child(ai_field(
                        palette,
                        t!("ai.proxyUsername"),
                        None,
                        self.existing_text_input_box("ai.input.proxy-username", false),
                    ))
                    .child(ai_field(
                        palette,
                        t!("ai.proxyPassword"),
                        Some(t!("ai.proxyPasswordDescription").into()),
                        self.existing_text_input_box("ai.input.proxy-password", true),
                    ))
                    .child(ai_field(
                        palette,
                        t!("ai.proxyBypass"),
                        Some(t!("ai.proxyBypassDescription").into()),
                        self.existing_text_input_box("ai.input.proxy-bypass", false),
                    ))
            });
        let general = div()
            .flex()
            .flex_col()
            .gap_4()
            .child(switches)
            .child(ai_field(
                palette,
                t!("ai.requestUserAgent"),
                Some(t!("ai.requestUserAgentDesc").into()),
                self.existing_text_input_box("ai.input.request-user-agent", false),
            ))
            .child(fields)
            .child(proxy_fields);
        let risk = match settings.agent_smart_auto_execute_max_risk {
            nyaterm_core::RiskLevel::Low => "low",
            nyaterm_core::RiskLevel::Medium => "medium",
            nyaterm_core::RiskLevel::High => "high",
            nyaterm_core::RiskLevel::Critical => "critical",
        };
        let risk = self.form_select_control(
            "ai-smart-risk",
            [
                ("low", "ai.riskLow"),
                ("medium", "ai.riskMedium"),
                ("high", "ai.riskHigh"),
                ("critical", "ai.riskCritical"),
            ]
            .into_iter()
            .map(|(value, label)| NyaSelectOption::new(value, t!(label)))
            .collect(),
            Some(risk.into()),
            false,
            cx,
        );
        let params = div()
            .grid()
            .grid_cols(if self.content_width >= 1024. { 2 } else { 1 })
            .gap_4()
            .child(ai_field(
                palette,
                t!("ai.agentMaxSteps"),
                None,
                self.existing_number_input_box("ai.number.agent-steps"),
            ))
            .child(ai_field(
                palette,
                t!("ai.agentStepTimeout"),
                None,
                self.existing_number_input_box("ai.number.agent-step-timeout-ms"),
            ))
            .child(ai_field(
                palette,
                t!("ai.terminalOutputLines"),
                None,
                self.existing_number_input_box("ai.number.terminal-output-lines"),
            ));
        let agent = div()
            .flex()
            .flex_col()
            .gap_4()
            .child(ai_field(
                palette,
                t!("ai.smartAutoExecuteMaxRisk"),
                Some(t!("ai.smartAutoExecuteMaxRiskDesc").into()),
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().min_w_0().child(risk))
                    .child(
                        div()
                            .id("ai-smart-risk-help")
                            .flex()
                            .items_center()
                            .child(
                                svg()
                                    .size(px(14.))
                                    .path("icons/menu/info.svg")
                                    .text_color(rgb(palette.text_dimmed)),
                            )
                            .tooltip(|window, cx| {
                                NyaTooltip::new(t!("ai.smartAutoExecuteRiskHelp")).build(window, cx)
                            }),
                    ),
            ))
            .child(params)
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(palette.text_muted))
                    .child(t!("ai.terminalOutputLinesDesc")),
            );
        div()
            .flex()
            .flex_col()
            .gap_5()
            .child(ai_card(
                palette,
                div().child(t!("ai.general")),
                div(),
                general,
            ))
            .child(ai_card(
                palette,
                div().child(t!("ai.agentSettings")),
                div(),
                agent,
            ))
    }
}
