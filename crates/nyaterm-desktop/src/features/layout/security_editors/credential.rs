use rust_i18n::t;

use std::borrow::Cow;

use gpui::{Context, IntoElement, KeyDownEvent, div, prelude::*, px, rgb};
use nyaterm_core::validate_prompt_regex;

use crate::features::{NyaTermApp, text_inputs::TextInputSetup};
use crate::models::SecurityCredentialEditorState;

use super::super::view_helpers::security_editor_field;

impl NyaTermApp {
    pub(in crate::features) fn security_credential_editor_view(
        &mut self,
        editor: SecurityCredentialEditorState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        // A stored secret is never shown, so the box says so in its
        // placeholder rather than standing a row of bullets in for it.
        let password_placeholder = if editor.has_password {
            t!("credentialManager.passwordUnchanged")
        } else {
            Cow::Borrowed("")
        };
        let username_regex_valid = editor.username_prompt_regex.trim().is_empty()
            || validate_prompt_regex(&editor.username_prompt_regex);
        let password_regex_valid = editor.password_prompt_regex.trim().is_empty()
            || validate_prompt_regex(&editor.password_prompt_regex);
        div()
            .flex()
            .flex_col()
            .gap_2()
            .track_focus(self.security.credential_editor_focus())
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.handle_security_credential_editor_key_down(event, window, cx);
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(div().flex_1().child(t!("credentialManager.enabled")))
                    .child(
                        nyaterm_ui::NyaSwitch::new("security-cred-enabled")
                            .checked(editor.enabled)
                            .disabled(self.security.editor_busy())
                            .tooltip(t!("credentialManager.enabled"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.toggle_security_credential_enabled(cx);
                            })),
                    ),
            )
            .child(security_editor_field(
                self,
                "cred-name",
                t!("credentialManager.nameLabel"),
                editor.name.clone(),
                TextInputSetup::default(),
                cx,
            ))
            .child(security_editor_field(
                self,
                "cred-user",
                t!("credentialManager.usernameLabel"),
                editor.username.clone(),
                TextInputSetup::default(),
                cx,
            ))
            .child(
                div()
                    .flex()
                    .items_end()
                    .gap_1()
                    .child(div().min_w_0().flex_1().child(security_editor_field(
                        self,
                        "cred-pass",
                        t!("credentialManager.passwordLabel"),
                        editor.password.expose_secret().to_owned(),
                        TextInputSetup {
                            placeholder: password_placeholder.into(),
                            masked: !editor.show_password,
                            multi_line: false,
                            submit_on_enter: false,
                            code: false,
                        },
                        cx,
                    )))
                    .child(
                        nyaterm_ui::NyaIconButton::new(
                            "security-cred-toggle-vis",
                            if editor.show_password {
                                "icons/eye-off.svg"
                            } else {
                                "icons/eye.svg"
                            },
                        )
                        .tooltip(t!(if editor.show_password {
                            "passwordManager.hidePassword"
                        } else {
                            "passwordManager.showPassword"
                        }))
                        .disabled(self.security.editor_busy())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.toggle_security_credential_editor_visibility(cx);
                        })),
                    ),
            )
            .child(security_editor_field(
                self,
                "cred-user-re",
                t!("credentialManager.promptRegexLabel"),
                editor.username_prompt_regex.clone(),
                TextInputSetup::default(),
                cx,
            ))
            .when(!username_regex_valid, |this| {
                this.child(
                    div()
                        .text_size(px(10.))
                        .text_color(rgb(palette.danger))
                        .child(t!("credentialManager.invalidRegex")),
                )
            })
            .child(security_editor_field(
                self,
                "cred-pass-re",
                t!("credentialManager.passwordRegexPlaceholder"),
                editor.password_prompt_regex.clone(),
                TextInputSetup::default(),
                cx,
            ))
            .when(!password_regex_valid, |this| {
                this.child(
                    div()
                        .text_size(px(10.))
                        .text_color(rgb(palette.danger))
                        .child(t!("credentialManager.invalidRegex")),
                )
            })
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
