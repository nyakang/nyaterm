use gpui::{AnyElement, Context, FontWeight, KeyDownEvent, SharedString, div, prelude::*, px, rgb};
use nyaterm_transport::SshCredentialPromptReason;
use nyaterm_transport::ssh_authentication::SshAuthenticationMethod;
use nyaterm_ui::{
    NYA_FORM_CONTROL_HEIGHT_PX, NyaButton, NyaButtonVariant, NyaCheckbox, NyaScrollArea,
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
        let reason = match state.prompt.reason {
            SshCredentialPromptReason::PasswordRejected => t!("sshAuth.passwordRejected"),
            SshCredentialPromptReason::PublickeyRequired => t!("sshAuth.publickeyRequired"),
            SshCredentialPromptReason::PublickeyRejected => t!("sshAuth.publickeyRejected"),
            SshCredentialPromptReason::KeyPassphraseRequired => t!("sshAuth.keyPassphraseRequired"),
            SshCredentialPromptReason::KeyPassphraseRejected => t!("sshAuth.keyPassphraseRejected"),
            _ => t!("sshAuth.missingPassword"),
        };
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
                    .text_xs()
                    .text_color(rgb(palette.text_muted))
                    .child(t!(
                        "sshAuth.description",
                        name = state.prompt.connection_name
                    )),
            )
            .child(
                div()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(palette.border))
                    .p_3()
                    .child(div().text_sm().child(format!(
                        "{}@{}:{}",
                        state.prompt.username, state.prompt.host, state.prompt.port
                    )))
                    .child(
                        div()
                            .mt_1()
                            .text_xs()
                            .text_color(rgb(palette.text_muted))
                            .child(reason),
                    ),
            );
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
                    NyaTabs::new("ssh-auth-method")
                        .items([
                            NyaTabItem::new(t!("sshAuth.passwordMethod")).disabled(form.busy),
                            NyaTabItem::new(t!("sshAuth.keyMethod")).disabled(form.busy),
                        ])
                        .selected_index(usize::from(form.method == AuthMethod::Key))
                        .on_select(cx.listener(|this, index, window, cx| {
                            this.change_runtime_auth_source(true, *index, window, cx)
                        })),
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
            let setup = if form.is_passphrase() {
                TextInputSetup::secret()
            } else {
                TextInputSetup::masked()
            };
            self.text_input(id.clone(), &state.value, setup.clone(), cx)
                .update(cx, |input, cx| input.set_disabled(form.busy, cx));
            content = content
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
            content = content.child(self.form_select_control(
                if key {
                    "ssh-auth-key"
                } else {
                    "ssh-auth-password"
                },
                options,
                selected,
                form.busy,
                cx,
            ));
            let mut actions = div().flex().gap_2().child(
                NyaButton::new("ssh-auth-refresh", t!("common.refresh"))
                    .small()
                    .disabled(form.busy)
                    .on_click(cx.listener(|this, _, _, cx| this.refresh_security_catalog(cx))),
            );
            if key {
                let selected = form.key_id.clone();
                actions = actions.child(
                    NyaButton::new("ssh-auth-manage", t!("sshAuth.manageKeys"))
                        .small()
                        .disabled(form.busy)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_security_key_editor(selected.clone(), window, cx)
                        })),
                );
            }
            content = content.child(actions);
        }
        if form.can_remember() {
            let mut remember = div()
                .debug_selector(|| "ssh-auth-remember-section".to_string())
                .flex()
                .flex_col()
                .gap_2()
                .rounded_md()
                .border_1()
                .border_color(rgb(palette.border))
                .p_3()
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

#[cfg(test)]
mod tests;
