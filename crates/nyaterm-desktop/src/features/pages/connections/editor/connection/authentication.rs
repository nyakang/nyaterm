use gpui::{Context, Div, div, prelude::*, px, rgb};
use nyaterm_core::ConnectionUsernameSource;
use nyaterm_ui::{NyaButton, NyaInputShell, NyaTabItem, NyaTabs};
use rust_i18n::t;

use super::super::super::list::{
    ConnectionEditorFields, ConnectionEditorRenderContext, connection_editor_select, editor_field,
    required,
};
use super::ConnectionEditorSectionContext;
use crate::features::NyaTermApp;
use crate::models::{
    ConnectionEditorCredentialOverlay, ConnectionEditorField, ConnectionEditorPasswordSource,
    ConnectionEditorSelect, ConnectionKindTab,
};

pub(super) fn login_identity(
    section: ConnectionEditorSectionContext<'_>,
    cx: &mut Context<NyaTermApp>,
) -> Div {
    let ConnectionEditorSectionContext {
        palette,
        editor,
        fields,
        ..
    } = section;
    let account_username = fields
        .account
        .as_ref()
        .map(|entry| entry.username.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let follows_account =
        editor.username_source == ConnectionUsernameSource::Account && account_username.is_some();
    if let Some(input) = fields.get(&ConnectionEditorField::Username) {
        let shown = if follows_account {
            account_username.clone().unwrap_or_default()
        } else {
            editor.username.clone()
        };
        input.update(cx, |state, cx| {
            state.set_readonly(follows_account, cx);
            if state.value(cx) != shown {
                state.set_content_silent(&shown, cx);
            }
        });
    }
    let id = editor.account_id.clone();
    let mut body = div()
        .debug_selector(|| "connection-login-identity".to_string())
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_end()
                .gap_2()
                .child(connection_editor_select(
                    ConnectionEditorRenderContext {
                        palette,
                        fields,
                        cx,
                    },
                    "connection-editor-account",
                    t!("dialog.account"),
                    ConnectionEditorSelect::Account,
                ))
                .child(
                    NyaButton::new("connection-account-new", t!("passwordManager.add"))
                        .form_control()
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_connection_account_editor(None, window, cx)
                        })),
                )
                .child(
                    NyaButton::new("connection-account-edit", t!("common.edit"))
                        .form_control()
                        .disabled(fields.account.is_none())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_connection_account_editor(id.clone(), window, cx)
                        })),
                ),
        );
    if editor.account_id.is_some() && fields.account.is_none() {
        body = body.child(
            div()
                .text_xs()
                .text_color(rgb(palette.danger))
                .child(t!("passwordManager.missingAccount")),
        );
    }
    let mut username_row = div().flex().flex_wrap().items_end().gap_2().child(
        div()
            .min_w_0()
            .flex_1()
            .flex()
            .flex_col()
            .child(editor_field(
                palette,
                if editor.kind == ConnectionKindTab::Ssh {
                    required(t!("dialog.username"))
                } else {
                    t!("credentialManager.usernameOptionalLabel").into()
                },
                ConnectionEditorField::Username,
                fields,
                cx,
            )),
    );
    if follows_account {
        username_row = username_row
            .debug_selector(|| "connection-account-username".to_string())
            .child(
                NyaButton::new("connection-username-override", t!("common.edit"))
                    .form_control()
                    .on_click(
                        cx.listener(|this, _, _, cx| this.use_connection_editor_username(cx)),
                    ),
            );
    } else if account_username.is_some()
        && editor.username_source == ConnectionUsernameSource::Connection
    {
        username_row = username_row.child(
            NyaButton::new(
                "connection-username-restore",
                t!("authForm.restoreAccountUsername"),
            )
            .form_control()
            .on_click(
                cx.listener(|this, _, _, cx| this.follow_connection_editor_account_username(cx)),
            ),
        );
    }
    body = body.child(username_row);
    if follows_account {
        body = body.child(
            div()
                .text_xs()
                .text_color(rgb(palette.text_muted))
                .child(t!("authForm.usernameProvidedByAccount")),
        );
    } else if account_username.is_some()
        && editor.username_source == ConnectionUsernameSource::Connection
    {
        body = body.child(
            div()
                .text_xs()
                .text_color(rgb(palette.text_muted))
                .child(t!("authForm.usernameForConnection")),
        );
    } else if fields.account.is_some() && account_username.is_none() {
        body = body.child(
            div()
                .text_xs()
                .text_color(rgb(palette.text_muted))
                .child(t!("authForm.accountUsernameFallbackDescription")),
        );
    }
    body
}

fn account_password_prompt(fields: &ConnectionEditorFields, has_password: bool) -> String {
    let Some(account) = fields.account.as_ref() else {
        return t!("passwordManager.missingAccount").to_string();
    };
    if has_password && fields.account_password_locked {
        return t!("authForm.passwordLocked").to_string();
    }
    let name = account.name.clone();
    if has_password {
        t!("authForm.accountPasswordDescription", name = name).to_string()
    } else {
        t!("authForm.accountPasswordMissingDescription", name = name).to_string()
    }
}

pub(super) fn password_source(
    section: ConnectionEditorSectionContext<'_>,
    cx: &mut Context<NyaTermApp>,
) -> Div {
    let ConnectionEditorSectionContext {
        palette,
        editor,
        fields,
        ..
    } = section;
    let uses_identity = matches!(
        editor.kind,
        ConnectionKindTab::Ssh | ConnectionKindTab::Telnet
    );
    let mut body = div()
        .debug_selector(|| "connection-password-controls".to_string())
        .flex()
        .flex_col()
        .gap_2();
    if !uses_identity {
        body = body.child(
            div()
                .flex()
                .items_end()
                .gap_2()
                .child(connection_editor_select(
                    ConnectionEditorRenderContext {
                        palette,
                        fields,
                        cx,
                    },
                    "connection-editor-saved-password",
                    t!("dialog.account"),
                    ConnectionEditorSelect::SavedPassword,
                ))
                .child(
                    NyaButton::new("connection-manage-accounts", t!("dialog.managePasswords"))
                        .form_control()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.set_connection_editor_credential_overlay(
                                Some(ConnectionEditorCredentialOverlay::Passwords),
                                cx,
                            )
                        })),
                ),
        );
    }
    if let Some(input) = fields.get(&ConnectionEditorField::Password) {
        body = body.child(
            div()
                .id("connection-password-field")
                .debug_selector(|| "connection-editor-field-Password".to_string())
                .anchor_scroll(
                    fields
                        .anchors
                        .get(&crate::models::ConnectionEditorFocusTarget::Field(
                            ConnectionEditorField::Password,
                        ))
                        .cloned(),
                )
                .w_full()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(palette.text_muted))
                        .child(t!("authForm.passwordOptional")),
                )
                .child(
                    div()
                        .w_full()
                        .h(px(super::super::super::list::EDITOR_CONTROL_HEIGHT_PX))
                        .child(
                            NyaInputShell::new("connection-password-input", input)
                                .height(px(super::super::super::list::EDITOR_CONTROL_HEIGHT_PX))
                                .on_mask_toggle(cx.listener(|this, _, window, cx| {
                                    this.toggle_connection_password_visibility(window, cx)
                                })),
                        ),
                ),
        );
    }
    if editor.password_account_id().is_some() {
        let message = if editor.password_source == ConnectionEditorPasswordSource::Direct {
            t!("authForm.connectionPasswordLegacy").to_string()
        } else {
            account_password_prompt(
                fields,
                fields
                    .account
                    .as_ref()
                    .is_some_and(|entry| entry.has_password),
            )
        };
        body = body.child(
            div()
                .text_xs()
                .text_color(rgb(palette.text_muted))
                .child(message),
        );
        if !uses_identity {
            body = body.child(
                div()
                    .text_xs()
                    .text_color(rgb(palette.text_muted))
                    .child(t!("authForm.passwordOnlyReference")),
            );
        }
    }
    if let Some(error) = fields.errors.get(&ConnectionEditorField::Password) {
        body = body.child(
            div()
                .text_xs()
                .text_color(rgb(palette.danger))
                .child(error.clone()),
        );
    }
    if editor.kind == ConnectionKindTab::Vnc && editor.password_account_id().is_none() {
        body = body.child(
            div()
                .text_xs()
                .text_color(rgb(palette.text_muted))
                .child(t!("dialog.vncPasswordLimit")),
        );
    }
    body
}

pub(super) fn password_authentication(
    section: ConnectionEditorSectionContext<'_>,
    cx: &mut Context<NyaTermApp>,
) -> Div {
    let editor = section.editor;
    let protocol_requires_no_auth =
        editor.kind == ConnectionKindTab::Vnc && editor.vnc_security.mode == "none";
    let body = div()
        .flex()
        .flex_col()
        .gap_2()
        .child(div().text_xs().child(t!("dialog.authentication")))
        .child(
            NyaTabs::new("connection-password-auth")
                .items([
                    NyaTabItem::new(t!("dialog.password")).disabled(protocol_requires_no_auth),
                    NyaTabItem::new(t!("dialog.noAuthentication")),
                ])
                .selected_index(usize::from(
                    editor.auth_mode == "none" || protocol_requires_no_auth,
                ))
                .on_select(cx.listener(|this, index, _, cx| {
                    this.set_connection_editor_select_value(
                        ConnectionEditorSelect::Authentication,
                        Some(if *index == 0 { "password" } else { "none" }),
                        cx,
                    )
                })),
        );
    if editor.auth_mode == "password"
        && !(editor.kind == ConnectionKindTab::Vnc && editor.vnc_security.mode == "none")
    {
        body.child(password_source(section, cx))
    } else {
        body
    }
}
