use gpui::{AnyElement, Context, FontWeight, KeyDownEvent, SharedString, div, prelude::*, px, rgb};
use nyaterm_transport::SshCredentialPromptReason;
use nyaterm_transport::ssh_authentication::SshAuthenticationMethod;
use nyaterm_ui::{
    NYA_FORM_CONTROL_HEIGHT_PX, NyaButton, NyaButtonVariant, NyaCheckbox, NyaScrollArea, NyaSelect,
    NyaSelectOption, NyaTabItem, NyaTabs,
};
use rust_i18n::t;

use super::state::{AuthMethod, SaveDestination};
use crate::features::root::SSH_AUTH_OVERLAY_PADDING_PX;
use crate::features::{NyaTermApp, session::CredentialPromptState, text_inputs::TextInputSetup};

const CARD_PADDING_PX: f32 = 24.;
const CARD_BORDER_PX: f32 = 1.;
const FOOTER_GAP_PX: f32 = 16.;
const ACTION_HEIGHT_PX: f32 = 32.;
const MAX_BODY_HEIGHT_PX: f32 = 550.;
const FOCUS_RING_PADDING_PX: f32 = 3.;

impl NyaTermApp {
    pub(in crate::features) fn runtime_authentication_view(
        &mut self,
        state: CredentialPromptState,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let form = state
            .authentication
            .as_ref()
            .expect("structured authentication form");
        let palette = self.theme_palette();
        // Use a pixel cap on the actual scroll viewport. Percentage heights in
        // this content-sized card create a cycle and collapse the form to zero.
        let reserved_height = 2. * SSH_AUTH_OVERLAY_PADDING_PX
            + 2. * (CARD_PADDING_PX + CARD_BORDER_PX)
            + FOOTER_GAP_PX
            + ACTION_HEIGHT_PX;
        let body_height =
            (self.shell.viewport_size().1 - reserved_height).clamp(0., MAX_BODY_HEIGHT_PX);
        let reason = authentication_notice(state.prompt.reason).map(|key| t!(key));
        let mut content = div()
            .debug_selector(|| "ssh-auth-content".to_string())
            .w_full()
            .min_w_0()
            .min_h_0()
            .p(px(FOCUS_RING_PADDING_PX))
            .flex_none()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .debug_selector(|| "ssh-auth-title".to_string())
                    .text_sm()
                    .font_weight(FontWeight(700.))
                    .child(t!("sshAuth.title")),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight(600.))
                            .child(state.prompt.connection_name.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(palette.text_muted))
                            .child(format!(
                                "{}@{}:{}",
                                state.prompt.username, state.prompt.host, state.prompt.port
                            )),
                    ),
            )
            .when_some(reason, |this, reason| {
                this.child(
                    div()
                        .debug_selector(|| "ssh-auth-notice".to_string())
                        .text_xs()
                        .text_color(rgb(palette.text_muted))
                        .child(reason),
                )
            });
        if !form.is_passphrase() {
            let password_available = form.request.available_methods.iter().any(|method| {
                matches!(
                    method,
                    SshAuthenticationMethod::Password
                        | SshAuthenticationMethod::KeyboardInteractive
                )
            });
            if password_available
                && form
                    .request
                    .available_methods
                    .contains(&SshAuthenticationMethod::Publickey)
            {
                content = content.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(div().text_xs().child(t!("sshAuth.authenticationMethod")))
                        .child(
                            NyaTabs::new("ssh-auth-method")
                                .items([
                                    NyaTabItem::new(t!("sshAuth.passwordMethod"))
                                        .disabled(form.busy),
                                    NyaTabItem::new(t!("sshAuth.keyMethod")).disabled(form.busy),
                                ])
                                .selected_index(usize::from(form.method == AuthMethod::Key))
                                .on_select(cx.listener(|this, index, window, cx| {
                                    this.change_runtime_auth_source(true, *index, window, cx)
                                })),
                        ),
                );
            }
            if form.method == AuthMethod::Password {
                content = content.child(
                    NyaTabs::new("ssh-auth-password-source")
                        .items([
                            NyaTabItem::new(t!("dialog.directPassword")).disabled(form.busy),
                            NyaTabItem::new(t!("dialog.savedPassword")).disabled(form.busy),
                        ])
                        .selected_index(usize::from(form.saved_password))
                        .on_select(cx.listener(|this, index, window, cx| {
                            this.change_runtime_auth_source(false, *index, window, cx)
                        })),
                );
            }
        }
        if form.uses_secret_input() {
            let id = super::super::credential_text_input_id(&state.id);
            let setup = TextInputSetup {
                placeholder: if form.is_passphrase() {
                    t!("sshAuth.keyPassphraseRequired").into()
                } else {
                    t!("sshAuth.missingPassword").into()
                },
                ..if form.is_passphrase() {
                    TextInputSetup::secret()
                } else {
                    TextInputSetup::masked()
                }
            };
            self.text_input(id.clone(), &state.value, setup.clone(), cx)
                .update(cx, |input, cx| input.set_disabled(form.busy, cx));
            content = content.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().text_xs().child(if form.is_passphrase() {
                        t!("sshAuth.passphrase")
                    } else {
                        t!("dialog.password")
                    }))
                    .child(
                        div()
                            .w_full()
                            .h(px(NYA_FORM_CONTROL_HEIGHT_PX))
                            .debug_selector(|| "ssh-auth-secret-field".to_string())
                            .child(self.text_input_box(id, &state.value, setup, cx)),
                    ),
            );
        } else {
            let key = form.method == AuthMethod::Key;
            let options: Vec<NyaSelectOption> = if key {
                self.security
                    .ssh_keys()
                    .iter()
                    .filter(|entry| entry.has_key_data)
                    .map(|entry| NyaSelectOption::new(entry.id.clone(), entry.name.clone()))
                    .collect()
            } else {
                self.security
                    .passwords()
                    .iter()
                    .filter(|entry| entry.has_password)
                    .map(|entry| {
                        NyaSelectOption::new(entry.id.clone(), entry.name.clone())
                            .subtitle(entry.username.clone())
                    })
                    .collect()
            };
            let selected = if key {
                form.key_id.clone()
            } else {
                form.password_id.clone()
            };
            let empty = options.is_empty();
            let loading = self.security.catalog_loading();
            let failed = self.security.catalog_load_failed();
            let placeholder = if key {
                t!("sshAuth.selectKey")
            } else {
                t!("sshAuth.selectSavedPassword")
            };
            let select = self.select_entity(
                if key {
                    "ssh-auth-key"
                } else {
                    "ssh-auth-password"
                },
                options,
                selected.clone(),
                form.busy || empty,
                cx,
            );
            select.update(cx, |select, cx| select.set_placeholder(placeholder, cx));
            let mut label =
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(div().text_xs().child(if key {
                        t!("sshAuth.savedKey")
                    } else {
                        t!("dialog.savedPassword")
                    }));
            if key {
                label = label.child(
                    div()
                        .flex_none()
                        .debug_selector(|| "ssh-auth-manage-action".to_string())
                        .child(
                            NyaButton::new(
                                "ssh-auth-manage",
                                if selected.is_some() {
                                    t!("sshAuth.editKey")
                                } else {
                                    t!("sshAuth.addKey")
                                },
                            )
                            .small()
                            .variant(NyaButtonVariant::Ghost)
                            .disabled(form.busy)
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.dismiss_send_command_selects("ssh-auth-", cx);
                                    this.open_security_key_editor(selected.clone(), window, cx)
                                },
                            )),
                        ),
                );
            }
            let hint = if loading {
                t!("common.loading")
            } else if failed {
                t!("sshAuth.catalogLoadFailed")
            } else if empty {
                if key {
                    t!("sshAuth.noKeys")
                } else {
                    t!("dialog.noPasswords")
                }
            } else if key {
                t!("sshAuth.keySelectionHint")
            } else {
                t!("sshAuth.selectSavedPassword")
            };
            content = content.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(label)
                    .child(
                        div()
                            .w_full()
                            .h(px(NYA_FORM_CONTROL_HEIGHT_PX))
                            .debug_selector(|| "ssh-auth-credential-select".to_string())
                            .child(NyaSelect::new(&select)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_xs()
                                    .text_color(rgb(palette.text_muted))
                                    .debug_selector(|| "ssh-auth-selection-hint".to_string())
                                    .child(hint),
                            )
                            .child(
                                NyaButton::new("ssh-auth-refresh", t!("common.refresh"))
                                    .small()
                                    .variant(NyaButtonVariant::Ghost)
                                    .disabled(form.busy || loading)
                                    .loading(loading)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.refresh_security_catalog(cx)
                                    })),
                            ),
                    ),
            );
        }
        if form.can_remember() {
            let mut remember = div()
                .debug_selector(|| "ssh-auth-remember-section".to_string())
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    NyaCheckbox::new("ssh-auth-remember")
                        .label(t!("sshAuth.rememberCredential"))
                        .checked(form.remember)
                        .disabled(form.busy)
                        .on_click(cx.listener(|this, checked, _, cx| {
                            if let Some(form) = this
                                .session
                                .prompts
                                .active_credential_mut()
                                .and_then(|s| s.authentication.as_mut())
                            {
                                form.remember = *checked;
                            }
                            cx.notify();
                        })),
                );
            if form.remember && !form.is_passphrase() && !form.saved_password {
                let mut options = vec![
                    NyaSelectOption::new("connection", t!("sshAuth.saveToConnection")),
                    NyaSelectOption::new("new", t!("sshAuth.createSavedPassword")),
                ];
                if form.context.account_id.is_some() {
                    options.push(NyaSelectOption::new(
                        "linked",
                        t!("sshAuth.updateSavedPassword"),
                    ));
                }
                let selected = match form.destination {
                    SaveDestination::Connection => "connection",
                    SaveDestination::NewAccount => "new",
                    SaveDestination::LinkedAccount => "linked",
                };
                remember = remember.child(self.form_select_control(
                    "ssh-auth-save",
                    options,
                    Some(selected.into()),
                    form.busy,
                    cx,
                ));
                if form.destination == SaveDestination::NewAccount {
                    self.text_input(
                        "ssh.auth.name",
                        &form.name,
                        TextInputSetup::placeholder(t!("sshAuth.savedPasswordName")),
                        cx,
                    )
                    .update(cx, |input, cx| input.set_disabled(form.busy, cx));
                    remember = remember.child(self.text_input_box(
                        "ssh.auth.name",
                        &form.name,
                        TextInputSetup::placeholder(t!("sshAuth.savedPasswordName")),
                        cx,
                    ));
                } else if form.destination == SaveDestination::LinkedAccount {
                    remember = remember.child(
                        div()
                            .text_xs()
                            .text_color(rgb(palette.text_muted))
                            .child(t!("sshAuth.sharedAccountWarning")),
                    );
                }
            }
            content = content.child(remember);
        }
        if let Some(error) = &form.error {
            content = content.child(
                div()
                    .debug_selector(|| "ssh-auth-error".to_string())
                    .text_xs()
                    .text_color(rgb(palette.text))
                    .child(error.clone()),
            );
        }
        let buttons =
            div()
                .debug_selector(|| "ssh-auth-footer".to_string())
                .h(px(ACTION_HEIGHT_PX))
                .flex_none()
                .flex()
                .justify_end()
                .gap_2()
                .child(
                    NyaButton::new("ssh-auth-cancel", t!("common.cancel"))
                        .height(px(ACTION_HEIGHT_PX))
                        .variant(NyaButtonVariant::Ghost)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.cancel_credential_prompt(window, cx)
                        })),
                )
                .child(
                    NyaButton::new("ssh-auth-submit", t!("sshAuth.submit"))
                        .height(px(ACTION_HEIGHT_PX))
                        .variant(NyaButtonVariant::Primary)
                        .disabled(!form.can_submit(&state.value))
                        .loading(form.busy)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.submit_runtime_authentication(window, cx)
                        })),
                );
        div()
            .debug_selector(|| "ssh-auth-card".to_string())
            .id(SharedString::from(format!(
                "credential-dialog-{}",
                state.id
            )))
            .w_full()
            .min_h_0()
            .rounded_md()
            .border(px(CARD_BORDER_PX))
            .border_color(rgb(palette.border))
            .bg(self.shell_surface_color(palette.bg))
            .shadow_lg()
            .p(px(CARD_PADDING_PX))
            .flex()
            .flex_col()
            .gap(px(FOOTER_GAP_PX))
            .on_click(|_, _, cx| cx.stop_propagation())
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if this.handle_credential_key_down(event, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .child(
                NyaScrollArea::new(format!("ssh-auth-body-{}", state.id))
                    .max_h(px(body_height))
                    .child(content),
            )
            .child(buttons)
            .into_any_element()
    }
}

// Server outcomes remain visible independently of the currently selected input.
// Missing credentials are explained by that input's label and placeholder.
fn authentication_notice(reason: SshCredentialPromptReason) -> Option<&'static str> {
    match reason {
        SshCredentialPromptReason::AdditionalPasswordRequired => {
            Some("sshAuth.additionalPasswordRequired")
        }
        SshCredentialPromptReason::PasswordRejected => Some("sshAuth.passwordRejected"),
        SshCredentialPromptReason::PublickeyRequired => Some("sshAuth.publickeyRequired"),
        SshCredentialPromptReason::PublickeyRejected
        | SshCredentialPromptReason::KeyRejectedPasswordFallback => {
            Some("sshAuth.publickeyRejected")
        }
        SshCredentialPromptReason::DockerElevation => Some("sshAuth.dockerElevation"),
        SshCredentialPromptReason::KeyPassphraseRequired => Some("sshAuth.keyPassphraseRequired"),
        SshCredentialPromptReason::KeyPassphraseRejected => Some("sshAuth.keyPassphraseRejected"),
        SshCredentialPromptReason::MissingPassword
        | SshCredentialPromptReason::KeyboardInteractive => None,
    }
}

#[cfg(test)]
mod tests;
