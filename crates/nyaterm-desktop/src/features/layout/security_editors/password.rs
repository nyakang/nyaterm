use rust_i18n::t;

use std::borrow::Cow;

use gpui::{Context, IntoElement, KeyDownEvent, div, prelude::*, px, rgb};

use crate::features::{NyaTermApp, text_inputs::TextInputSetup};
use crate::models::SecurityPasswordEditorState;

use super::super::view_helpers::security_editor_field;

impl NyaTermApp {
    pub(in crate::features) fn security_password_editor_view(
        &mut self,
        editor: SecurityPasswordEditorState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        // A stored secret is never shown, so the box says so in its
        // placeholder rather than standing a row of bullets in for it. The
        // reveal toggle now unmasks the box itself.
        let password_placeholder = if editor.has_password {
            t!("passwordManager.passwordUnchanged")
        } else {
            Cow::Borrowed("")
        };
        let password_masked = !editor.show_password;
        div()
            .flex()
            .flex_col()
            .gap_2()
            .track_focus(self.security.password_editor_focus())
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.handle_security_password_editor_key_down(event, window, cx);
            }))
            .child(security_editor_field(
                self,
                "pw-name",
                t!("passwordManager.nameLabel"),
                editor.name.clone(),
                TextInputSetup::default(),
                cx,
            ))
            .child(security_editor_field(
                self,
                "pw-username",
                t!("passwordManager.usernameLabel"),
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
                        "pw-value",
                        t!("passwordManager.passwordLabel"),
                        editor.password.expose_secret().to_owned(),
                        TextInputSetup {
                            placeholder: password_placeholder.into(),
                            masked: password_masked,
                            multi_line: false,
                            submit_on_enter: false,
                            code: false,
                        },
                        cx,
                    )))
                    .child(
                        nyaterm_ui::NyaIconButton::new(
                            "security-pw-toggle-vis",
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
                            this.toggle_security_password_editor_visibility(cx);
                        })),
                    ),
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
