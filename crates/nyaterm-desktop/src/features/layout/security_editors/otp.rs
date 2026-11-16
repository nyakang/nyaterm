use rust_i18n::t;

use std::borrow::Cow;

use gpui::{Context, IntoElement, KeyDownEvent, div, prelude::*, px, rgb};
use nyaterm_core::validate_prompt_regex;
use nyaterm_ui::NyaNumberInputOptions;

use crate::features::{NyaTermApp, text_inputs::TextInputSetup};
use crate::models::SecurityOtpEditorState;

use super::super::view_helpers::{
    security_editor_field, security_number_editor_field, security_type_chip,
};

impl NyaTermApp {
    pub(in crate::features) fn security_otp_editor_view(
        &mut self,
        editor: SecurityOtpEditorState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        // A stored secret is never shown, so the box says so in its
        // placeholder rather than standing a row of bullets in for it.
        let secret_placeholder = if editor.has_secret {
            t!("otpManager.secretUnchanged")
        } else {
            Cow::Borrowed("")
        };
        let prompt_regex_valid =
            editor.prompt_regex.trim().is_empty() || validate_prompt_regex(&editor.prompt_regex);

        div()
            .flex()
            .flex_col()
            .gap_2()
            .track_focus(self.security.otp_editor_focus())
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.handle_security_otp_editor_key_down(event, window, cx);
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(security_type_chip(
                        palette,
                        "TOTP",
                        editor.otp_type != "hotp",
                        self.security.editor_busy(),
                        cx.listener(|this, _, _, cx| {
                            this.set_security_otp_type("totp", cx);
                        }),
                    ))
                    .child(security_type_chip(
                        palette,
                        "HOTP",
                        editor.otp_type == "hotp",
                        self.security.editor_busy(),
                        cx.listener(|this, _, _, cx| {
                            this.set_security_otp_type("hotp", cx);
                        }),
                    ))
                    .child(div().flex_1())
                    .child(
                        div().w(px(130.)).child(
                            self.form_select_control(
                                "security-otp-algorithm",
                                ["SHA1", "SHA256", "SHA512"]
                                    .into_iter()
                                    .map(|value| nyaterm_ui::NyaSelectOption::new(value, value))
                                    .collect(),
                                Some(editor.algorithm.clone()),
                                self.security.editor_busy(),
                                cx,
                            ),
                        ),
                    ),
            )
            .child(security_editor_field(
                self,
                "otp-issuer",
                t!("otpManager.issuerLabel"),
                editor.issuer.clone(),
                TextInputSetup::default(),
                cx,
            ))
            .child(security_editor_field(
                self,
                "otp-username",
                t!("otpManager.usernameLabel"),
                editor.username.clone(),
                TextInputSetup::default(),
                cx,
            ))
            .child(security_editor_field(
                self,
                "otp-prompt-regex",
                t!("otpManager.promptRegex"),
                editor.prompt_regex.clone(),
                TextInputSetup::placeholder(t!("otpManager.promptRegexPlaceholder")),
                cx,
            ))
            .when(!prompt_regex_valid, |this| {
                this.child(
                    div()
                        .text_size(px(10.))
                        .text_color(rgb(palette.danger))
                        .child(t!("credentialManager.invalidRegex")),
                )
            })
            .child(security_editor_field(
                self,
                "otp-secret",
                t!("otpManager.secretLabel"),
                editor.secret.expose_secret().to_owned(),
                TextInputSetup {
                    placeholder: secret_placeholder.into(),
                    masked: true,
                    kind: nyaterm_ui::NyaInputKind::Secret,
                    multi_line: false,
                    submit_on_enter: false,
                    code: false,
                },
                cx,
            ))
            .child(
                div()
                    .grid()
                    .grid_cols(3)
                    .gap_2()
                    .child(security_number_editor_field(
                        self,
                        "otp-digits",
                        t!("otpManager.digits"),
                        editor.digits.clone(),
                        NyaNumberInputOptions::default().range(4.0, 10.0).step(1.0),
                        cx,
                    ))
                    .child(security_number_editor_field(
                        self,
                        "otp-period",
                        t!("otpManager.period"),
                        editor.period.clone(),
                        NyaNumberInputOptions::default()
                            .range(1.0, 3600.0)
                            .step(1.0),
                        cx,
                    ))
                    .child(security_number_editor_field(
                        self,
                        "otp-counter",
                        t!("otpManager.counter"),
                        editor.counter.clone(),
                        NyaNumberInputOptions::default()
                            .range(0.0, i64::MAX as f64)
                            .step(1.0),
                        cx,
                    )),
            )
            .when_some(editor.error.clone(), |this, error| {
                this.child(
                    div()
                        .text_size(px(10.))
                        .text_color(rgb(palette.danger))
                        .child(error),
                )
            })
    }
}
