use rust_i18n::t;

use std::borrow::Cow;

use gpui::{
    Context, FontWeight, IntoElement, KeyDownEvent, SharedString, div, prelude::*, px, rgb, rgba,
};
use nyaterm_transport::{
    SshAgentPromptAction, SshAgentPromptPhase, SshCredentialPromptKind, SshCredentialPromptReason,
};
use nyaterm_ui::NyaScrollable;

use crate::features::session::{
    AgentPromptRequest, AgentPromptState, CredentialPromptState, HostKeyPromptChoice,
    HostKeyPromptIssue, HostKeyPromptRequest, KeyboardInteractivePromptState,
    credential_prompt_target, credential_text_input_id, keyboard_interactive_text_input_id,
    unix_seconds_now,
};
use crate::features::view_widgets::dialog_action_button;
use crate::features::{NyaTermApp, text_inputs::TextInputSetup};
use crate::widgets::small_button;

impl NyaTermApp {
    pub(in crate::features) fn agent_prompt_banner(
        &mut self,
        request: AgentPromptRequest,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let snapshot = request.snapshot();
        let pending = snapshot.state == AgentPromptState::Pending;
        let retryable = !pending;
        let cancel_id = request.id.clone();
        let retry_id = request.id.clone();
        let title = if pending {
            t!("sshAuth.agentWaitingTitle")
        } else {
            t!("sshAuth.agentFailedTitle")
        };
        let phase = if pending {
            t!("sshAuth.agentApprovalWaiting")
        } else {
            match snapshot.prompt.phase {
                SshAgentPromptPhase::Connect => t!("sshAuth.agentConnectFailed"),
                SshAgentPromptPhase::ListIdentities => t!("sshAuth.agentIdentitiesFailed"),
                SshAgentPromptPhase::Sign => t!("sshAuth.agentApprovalRequired"),
            }
        };
        let target = format!(
            "{}@{}:{}",
            snapshot.prompt.username, snapshot.prompt.host, snapshot.prompt.port
        );
        div()
            .id(SharedString::from(format!("agent-dialog-{}", request.id)))
            .w_full()
            .rounded_md()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(self.shell_surface_color(palette.bg))
            .shadow_lg()
            .p_6()
            .flex()
            .flex_col()
            .gap_4()
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().text_sm().font_weight(FontWeight(700.)).child(title))
                    .child(div().text_xs().text_color(rgb(palette.text)).child(target))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(palette.text_muted))
                            .child(phase),
                    )
                    .when(!pending, |this| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(rgb(palette.text_muted))
                                .child(snapshot.prompt.message.clone()),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(small_button(
                        palette,
                        format!("agent-cancel-{}", request.id),
                        t!("common.cancel"),
                        cx.listener(move |this, _, _, cx| {
                            this.resolve_agent_prompt(
                                cancel_id.clone(),
                                SshAgentPromptAction::Cancel,
                                cx,
                            );
                        }),
                    ))
                    .when(retryable, |this| {
                        this.child(dialog_action_button(
                            palette,
                            format!("agent-retry-{}", request.id),
                            t!("common.retry"),
                            false,
                            cx.listener(move |this, _, _, cx| {
                                this.resolve_agent_prompt(
                                    retry_id.clone(),
                                    SshAgentPromptAction::Retry,
                                    cx,
                                );
                            }),
                        ))
                    }),
            )
    }

    pub(in crate::features) fn host_key_prompt_banner(
        &mut self,
        prompt: HostKeyPromptRequest,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let accept_id = prompt.id.clone();
        let reject_id = prompt.id.clone();
        let changed = matches!(prompt.issue, HostKeyPromptIssue::Changed);
        let description = match prompt.issue {
            HostKeyPromptIssue::Unknown => t!("settings.hostKeyVerifyNew"),
            HostKeyPromptIssue::Changed => t!("settings.hostKeyVerifyChanged"),
        };
        let detail_row = |label: Cow<'static, str>, value: String| {
            div()
                .flex()
                .items_start()
                .gap_3()
                .text_xs()
                .child(
                    div()
                        .w(px(88.))
                        .flex_none()
                        .text_color(rgb(palette.text_muted))
                        .child(label),
                )
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .font_family(crate::features::shell::gpui_code_font_family())
                        .text_color(rgb(palette.text))
                        .child(value),
                )
        };

        div()
            .w_full()
            .max_w(px(384.))
            .mx_auto()
            .rounded_md()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(self.shell_surface_color(palette.bg))
            .shadow_lg()
            .p_6()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight(700.))
                            .text_color(rgb(palette.text))
                            .child(t!("settings.hostKeyVerifyTitle")),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(palette.text_muted))
                            .child(description),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(detail_row(
                        t!("settings.hostKeyVerifyHost"),
                        prompt.host_key.host_identifier.clone(),
                    ))
                    .child(detail_row(
                        t!("settings.hostKeyVerifyKeyType"),
                        prompt.host_key.key_type.clone(),
                    ))
                    .child(detail_row(
                        t!("settings.hostKeyVerifyFingerprint"),
                        prompt.host_key.fingerprint.clone(),
                    )),
            )
            .when(changed, |this| {
                this.child(
                    div()
                        .rounded_sm()
                        .border_1()
                        .border_color(rgba((palette.danger << 8) | 0x80))
                        .bg(rgba((palette.danger << 8) | 0x1a))
                        .p_2()
                        .text_size(px(11.))
                        .text_color(rgb(palette.danger))
                        .child(t!("settings.hostKeyVerifyWarning")),
                )
            })
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(small_button(
                        palette,
                        format!("host-key-reject-{reject_id}"),
                        t!("settings.hostKeyVerifyReject"),
                        cx.listener(move |this, _, _, cx| {
                            this.resolve_host_key_prompt(
                                reject_id.clone(),
                                HostKeyPromptChoice::Reject,
                                cx,
                            );
                        }),
                    ))
                    .child(dialog_action_button(
                        palette,
                        format!("host-key-accept-{accept_id}"),
                        t!("settings.hostKeyVerifyAccept"),
                        changed,
                        cx.listener(move |this, _, _, cx| {
                            this.resolve_host_key_prompt(
                                accept_id.clone(),
                                HostKeyPromptChoice::Accept,
                                cx,
                            );
                        }),
                    )),
            )
    }

    pub(in crate::features) fn credential_prompt_banner(
        &mut self,
        prompt: CredentialPromptState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let title = match prompt.prompt.kind {
            SshCredentialPromptKind::Password => t!("runtimePrompt.sshPassword"),
            SshCredentialPromptKind::KeyPassphrase => t!("runtimePrompt.sshKeyPassphrase"),
            SshCredentialPromptKind::KeyboardInteractive => {
                t!("runtimePrompt.sshVerification")
            }
        };
        let reason = match prompt.prompt.reason {
            SshCredentialPromptReason::KeyRejectedPasswordFallback => {
                t!("sshAuth.keyRejectedPasswordFallback")
            }
            SshCredentialPromptReason::DockerElevation => t!("sshAuth.dockerElevation"),
            SshCredentialPromptReason::MissingPassword => t!("sshAuth.missingPassword"),
            SshCredentialPromptReason::PasswordRejected => t!("sshAuth.passwordRejected"),
            SshCredentialPromptReason::KeyPassphraseRequired => {
                t!("sshAuth.keyPassphraseRequired")
            }
            SshCredentialPromptReason::KeyboardInteractive => {
                t!("runtimePrompt.keyboardInteractive")
            }
        };
        let input_id = credential_text_input_id(&prompt.id);
        let input_setup = if prompt.prompt.echo {
            TextInputSetup::default()
        } else {
            TextInputSetup::masked()
        };
        let input = self.text_input_box(input_id, &prompt.value, input_setup, cx);
        let mut details = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(div().text_sm().font_weight(FontWeight(700.)).child(title))
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(palette.text))
                    .child(credential_prompt_target(&prompt.prompt)),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(palette.text_muted))
                    .child(reason),
            );
        if let Some(prompt_text) = prompt
            .prompt
            .prompt_text
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            details = details.child(
                div()
                    .mt_1()
                    .text_xs()
                    .text_color(rgb(palette.text))
                    .child(prompt_text.to_string()),
            );
        }

        div()
            .id(SharedString::from(format!(
                "credential-dialog-{}",
                prompt.id
            )))
            .w_full()
            .rounded_md()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(self.shell_surface_color(palette.bg))
            .shadow_lg()
            .p_6()
            .flex()
            .flex_col()
            .gap_4()
            .on_click(|_, _, cx| cx.stop_propagation())
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if this.handle_credential_key_down(event, cx) {
                    cx.stop_propagation();
                }
            }))
            .child(details)
            .child(input)
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(small_button(
                        palette,
                        format!("credential-cancel-{}", prompt.id),
                        t!("common.cancel"),
                        cx.listener(|this, _, _, cx| {
                            this.cancel_credential_prompt(cx);
                        }),
                    ))
                    .child(dialog_action_button(
                        palette,
                        format!("credential-submit-{}", prompt.id),
                        t!("sshAuth.submit"),
                        false,
                        cx.listener(|this, _, _, cx| {
                            this.submit_credential_prompt(cx);
                        }),
                    )),
            )
    }

    pub(in crate::features) fn keyboard_interactive_prompt_banner(
        &mut self,
        prompt: KeyboardInteractivePromptState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let title = if prompt.request.round > 1 {
            t!("otp.titleWithRound", round = prompt.request.round).to_string()
        } else {
            t!("otp.title").to_string()
        };
        let description = t!("otp.description", name = prompt.request.connection_name);
        let challenge_title = prompt.request.name.trim();
        let challenge_instructions = prompt.request.instructions.trim();
        let mut fields = div().min_w_0().flex().flex_col().gap_3();

        if !challenge_title.is_empty() || !challenge_instructions.is_empty() {
            let mut challenge = div()
                .min_w_0()
                .rounded_md()
                .border_1()
                .border_color(rgb(palette.border))
                .bg(rgba((palette.surface_elevated << 8) | 0x4d))
                .px_3()
                .py_2();
            if !challenge_title.is_empty() {
                challenge = challenge.child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight(600.))
                        .text_color(rgb(palette.text))
                        .child(challenge_title.to_string()),
                );
            }
            if !challenge_instructions.is_empty() {
                challenge = challenge.child(
                    div()
                        .when(!challenge_title.is_empty(), |this| this.mt_1())
                        .text_xs()
                        .text_color(rgb(palette.text_muted))
                        .child(challenge_instructions.to_string()),
                );
            }
            fields = fields.child(challenge);
        }

        for (index, field) in prompt.request.prompts.iter().enumerate() {
            let value = prompt.responses.get(index).cloned().unwrap_or_default();
            let setup = if field.echo {
                TextInputSetup::default()
            } else {
                TextInputSetup::masked()
            };
            let request_id = prompt.id.clone();
            let field_id = keyboard_interactive_text_input_id(&request_id, index);
            let click_listener = cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.session
                    .prompt_focus_keyboard_interactive_response(&request_id, index);
                cx.notify();
            });
            let input = self.text_input_box(field_id.clone(), &value, setup, cx);
            fields = fields.child(
                div()
                    .min_w_0()
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(palette.text_muted))
                            .child(keyboard_interactive_prompt_label(&field.prompt, index)),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("{field_id}.click-target")))
                            .mt_1()
                            .on_click(click_listener)
                            .child(input),
                    ),
            );
        }

        if prompt.request.otp_id.is_some() {
            let has_code = prompt.otp_code.is_some();
            let is_hotp = prompt.otp_type.as_deref() == Some("hotp");
            let is_totp = prompt.otp_type.as_deref() == Some("totp");
            let otp_type_label = prompt
                .otp_type
                .as_deref()
                .map(str::to_ascii_uppercase)
                .unwrap_or_else(|| "OTP".to_string());
            let remaining = if is_totp && prompt.otp_period > 0 {
                let period = prompt.otp_period.max(1);
                period - (unix_seconds_now() % period)
            } else {
                0
            };
            let display_code = prompt
                .otp_code
                .as_deref()
                .map(format_keyboard_interactive_otp_code)
                .unwrap_or_else(|| "--- ---".to_string());
            let mut otp_panel = div()
                .min_w_0()
                .rounded_md()
                .border_1()
                .border_color(rgb(palette.border))
                .bg(rgba((palette.surface_elevated << 8) | 0x4d))
                .p_3()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(rgb(palette.text_muted))
                                .child(t!("otp.currentCode")),
                        )
                        .child(
                            div()
                                .rounded_full()
                                .border_1()
                                .border_color(rgb(palette.border))
                                .px_2()
                                .py(px(2.))
                                .text_size(px(9.))
                                .font_weight(FontWeight(600.))
                                .text_color(rgb(palette.text_muted))
                                .child(otp_type_label),
                        ),
                )
                .child(
                    div()
                        .font_family(crate::features::shell::gpui_code_font_family())
                        .text_size(px(18.))
                        .font_weight(FontWeight(600.))
                        .text_color(rgb(palette.text))
                        .child(display_code),
                );
            if is_totp && has_code {
                otp_panel = otp_panel.child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(if remaining <= 5 {
                            palette.warning
                        } else {
                            palette.text_muted
                        }))
                        .child(t!("otp.expiresIn", seconds = remaining)),
                );
            } else if is_hotp && !has_code {
                otp_panel = otp_panel.child(
                    div()
                        .text_xs()
                        .text_color(rgb(palette.text_muted))
                        .child(t!("otp.hotpHint")),
                );
            }
            if let Some(error) = prompt.otp_error.as_ref() {
                otp_panel = otp_panel.child(
                    div()
                        .text_xs()
                        .text_color(rgb(palette.danger))
                        .child(error.clone()),
                );
            }
            otp_panel = otp_panel.child({
                let mut actions = div().flex().flex_wrap().gap_2();
                if is_hotp || !has_code {
                    actions = actions.child(small_button(
                        palette,
                        format!("keyboard-interactive-otp-generate-{}", prompt.id),
                        t!("otp.generateCode"),
                        cx.listener(|this, _, _, cx| {
                            this.generate_keyboard_interactive_otp_code(cx);
                        }),
                    ));
                }
                if has_code {
                    actions = actions.child(small_button(
                        palette,
                        format!("keyboard-interactive-otp-copy-{}", prompt.id),
                        t!("otp.copyCode"),
                        cx.listener(|this, _, _, cx| {
                            this.copy_keyboard_interactive_otp_code(cx);
                        }),
                    ));
                    actions = actions.child(dialog_action_button(
                        palette,
                        format!("keyboard-interactive-otp-send-{}", prompt.id),
                        t!("otp.sendToInput"),
                        false,
                        cx.listener(|this, _, _, cx| {
                            this.send_keyboard_interactive_otp_to_input(cx);
                        }),
                    ));
                }
                actions
            });
            fields = fields.child(otp_panel);
        }

        div()
            .id(SharedString::from(format!(
                "keyboard-interactive-dialog-{}",
                prompt.id
            )))
            .w_full()
            .max_h(px((self.shell.viewport_size().1 - 32.).max(240.)))
            .rounded_md()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(self.shell_surface_color(palette.bg))
            .shadow_lg()
            .p_6()
            .flex()
            .flex_col()
            .gap_4()
            .on_click(|_, _, cx| cx.stop_propagation())
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if this.handle_keyboard_interactive_key_down(event, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .overflow_y_scrollbar()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().text_sm().font_weight(FontWeight(700.)).child(title))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(palette.text_muted))
                            .child(description),
                    ),
            )
            .child(fields)
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(small_button(
                        palette,
                        format!("keyboard-interactive-cancel-{}", prompt.id),
                        t!("otp.cancel"),
                        cx.listener(|this, _, _, cx| {
                            this.cancel_keyboard_interactive_prompt(cx);
                        }),
                    ))
                    .child(dialog_action_button(
                        palette,
                        format!("keyboard-interactive-submit-{}", prompt.id),
                        t!("otp.submit"),
                        false,
                        cx.listener(|this, _, _, cx| {
                            this.submit_keyboard_interactive_prompt(cx);
                        }),
                    )),
            )
    }
}

fn keyboard_interactive_prompt_label(prompt: &str, index: usize) -> String {
    let label = prompt.trim().trim_end_matches(':').trim();
    if label.is_empty() {
        format!("Response {}", index + 1)
    } else {
        label.to_string()
    }
}

fn format_keyboard_interactive_otp_code(code: &str) -> String {
    if code.chars().count() == 6 {
        let mut chars = code.chars();
        let first = chars.by_ref().take(3).collect::<String>();
        format!("{first} {}", chars.collect::<String>())
    } else {
        code.to_string()
    }
}

#[cfg(test)]
mod keyboard_interactive_prompt_tests {
    use super::{format_keyboard_interactive_otp_code, keyboard_interactive_prompt_label};

    #[test]
    fn prompt_labels_remove_only_trailing_colons() {
        assert_eq!(keyboard_interactive_prompt_label(" Code:  ", 0), "Code");
        assert_eq!(keyboard_interactive_prompt_label("", 1), "Response 2");
        assert_eq!(
            keyboard_interactive_prompt_label("Challenge: response", 0),
            "Challenge: response"
        );
    }

    #[test]
    fn six_digit_otp_codes_are_grouped_for_display() {
        assert_eq!(format_keyboard_interactive_otp_code("123456"), "123 456");
        assert_eq!(format_keyboard_interactive_otp_code("12345678"), "12345678");
    }
}
