use crate::features::pages::settings::panel::SettingsPanel;
use gpui::{Context, IntoElement, div, prelude::*, px, rgb};
use nyaterm_core::ai::mcp_client::{ConfigTransport, McpClient, client_config};
use nyaterm_core::ai::mcp_http::McpTransport;
use nyaterm_ui::{NyaButton, NyaScrollable, NyaSelectOption};
use rust_i18n::t;

impl SettingsPanel {
    pub(in crate::features::pages::settings) fn mcp_setup_content(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let settings = self.ai.settings_config().external_mcp.clone();
        let client = settings.selected_client().clone();
        let http = self.http_mcp_view(cx);
        let mut options: Vec<_> = McpClient::SUPPORTED
            .into_iter()
            .filter(|item| item.supports(&settings.transport))
            .map(|item| NyaSelectOption::new(item.as_str().to_owned(), item.label()))
            .collect();
        if !client.supports(&settings.transport) {
            options.push(NyaSelectOption::new(
                client.as_str().to_owned(),
                t!("ai.mcp.setup.unsupportedClient"),
            ));
        }
        let selector = self.form_select_control(
            "ai-mcp-client",
            options,
            Some(client.as_str().to_owned()),
            false,
            cx,
        );
        // Preview generation never reads the credential. Only the copy action does.
        let preview = if matches!(settings.transport, McpTransport::Unknown(_)) {
            Err(t!("ai.mcp.unsupportedTransport").to_string())
        } else if !client.supports(&settings.transport) {
            Err(t!("ai.mcp.setup.unsupportedClient").to_string())
        } else {
            match &settings.transport {
                McpTransport::StreamableHttp if http.running => http
                    .port
                    .ok_or_else(|| t!("ai.mcp.unavailable").to_string())
                    .and_then(|port| {
                        client_config(
                            &client,
                            ConfigTransport::Http {
                                port,
                                token: "********",
                            },
                        )
                        .map_err(str::to_owned)
                    }),
                McpTransport::StreamableHttp => Err(t!(http.status_key).to_string()),
                McpTransport::Stdio => self
                    .ai
                    .agent_management
                    .mcp_helper_path
                    .as_ref()
                    .ok_or_else(|| {
                        if self.ai.agent_management.helper_checked {
                            t!("ai.mcp.helperMissing").to_string()
                        } else {
                            t!("ai.acp.helperChecking").to_string()
                        }
                    })
                    .and_then(|path| {
                        client_config(
                            &client,
                            ConfigTransport::Stdio {
                                command: &path.to_string_lossy(),
                            },
                        )
                        .map_err(str::to_owned)
                    }),
                McpTransport::Unknown(_) => Err(t!("ai.mcp.unsupportedTransport").to_string()),
            }
        };
        let ready = preview.is_ok();
        let hint = if matches!(client, McpClient::Unknown(_)) {
            t!("ai.mcp.setup.unsupportedClient").to_string()
        } else {
            let key = format!("ai.mcp.setup.hints.{}", client.as_str());
            t!(&key).to_string()
        };
        let format = if client == McpClient::Codex {
            "TOML"
        } else {
            "JSON"
        };
        let copy_client = client.as_str().to_owned();
        div()
            .flex()
            .flex_col()
            .gap_3()
            .min_w_0()
            .child(div().text_sm().child(t!("ai.mcp.setup.title")))
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .debug_selector(|| "ai-mcp-client".into())
                    .child(selector),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(palette.text_muted))
                    .child(hint),
            )
            .when(settings.transport == McpTransport::Stdio, |this| {
                this.child(div().text_size(px(12.)).child(t!("ai.mcp.setup.stdioHint")))
            })
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(format)
                    .child(
                        NyaButton::new("ai-mcp-copy-config", t!("ai.mcp.copyConfig"))
                            .disabled(!ready)
                            .on_click(cx.listener(move |panel, _, _, cx| {
                                panel.copy_external_mcp_config(&copy_client, cx)
                            })),
                    ),
            )
            .child(
                div()
                    .debug_selector(|| "ai-mcp-config-preview".into())
                    .w_full()
                    .min_w_0()
                    .child(
                        div()
                            .id("ai-mcp-config-preview")
                            .w_full()
                            .min_w_0()
                            .max_h(px(300.))
                            .p_3()
                            .rounded_md()
                            .bg(rgb(palette.surface))
                            .text_size(px(12.))
                            .font_family("monospace")
                            .overflow_scrollbar()
                            .child(preview.unwrap_or_else(|error| error)),
                    ),
            )
            .when(settings.transport == McpTransport::StreamableHttp, |this| {
                this.child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(palette.text_muted))
                        .child(t!("ai.mcp.setup.maskedHint")),
                )
            })
    }
}
