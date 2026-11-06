use std::sync::Arc;

use gpui::{Context, Window};
use nyaterm_core::SecretString;
use nyaterm_core::models::ssh_runtime_auth::SshCredentialSaveIntent;
use nyaterm_store::{StoreDomain, runtime_auth::PreparedCredentialSave};
use nyaterm_transport::SshKeyAuthConfig;
use nyaterm_transport::ssh_authentication::{SshAuthenticationMaterial, SshAuthenticationResponse};
use rust_i18n::t;

use super::state::{AuthMethod, RuntimeAuthenticationForm, SaveDestination};
use crate::features::{NyaTermApp, runtime_jobs::await_blocking_result};

impl NyaTermApp {
    pub(in crate::features) fn submit_runtime_authentication(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.session.prompts.active_credential().cloned() else {
            return;
        };
        let Some(form) = state.authentication else {
            return;
        };
        if !form.can_submit(&state.value) {
            return;
        }
        if let Some(active) = self
            .session
            .prompts
            .active_credential_mut()
            .and_then(|s| s.authentication.as_mut())
        {
            active.busy = true;
            active.error = None;
        }
        let store = self.store_blocking_client();
        let scheduler = self.blocking_jobs.clone();
        let request_id = state.id;
        let context = Arc::clone(&form.context);
        let response_tx = state.response_tx;
        let secret: SecretString = state.value.into();
        cx.spawn_in(window, async move |this, cx| {
            let task = scheduler.submit_task("ssh-auth-material", move |_| {
                form.context.attempt.check()?;
                store
                    .request_fn(StoreDomain::Security, move |store| {
                        prepare_response(store, &form, secret)
                    })
                    .map_err(|e| e.to_string())
            });
            let result = await_blocking_result(task).await;
            let mut submitted = false;
            let _ = this.update(cx, |this, cx| {
                if context.attempt.is_cancelled()
                    || !this
                        .session
                        .prompts
                        .active_credential()
                        .is_some_and(|s| s.id == request_id)
                {
                    return;
                }
                match result {
                    Ok((response, key_id, save)) => {
                        let id = response
                            .response_id
                            .clone()
                            .expect("runtime responses have provenance");
                        if context
                            .remember_selection(
                                id,
                                key_id,
                                match &response.material {
                                    SshAuthenticationMaterial::Key(key) => Some(key.clone()),
                                    _ => None,
                                },
                                save,
                            )
                            .is_ok()
                            && response_tx.send_authentication(response).is_ok()
                        {
                            this.session.prompts.take_credential();
                            this.forget_text_inputs("ssh.credential.");
                            this.forget_text_inputs("ssh.auth.");
                            submitted = true;
                        } else {
                            context.finish(None);
                            this.session.prompts.take_credential();
                            this.forget_text_inputs("ssh.credential.");
                            this.forget_text_inputs("ssh.auth.");
                        }
                    }
                    Err(_) => {
                        if let Some(form) = this
                            .session
                            .prompts
                            .active_credential_mut()
                            .and_then(|s| s.authentication.as_mut())
                        {
                            form.busy = false;
                            form.error = Some(t!("sshAuth.credentialUnavailable").to_string());
                        }
                        this.session.prompt_restore_credential_focus();
                    }
                }
                cx.notify();
            });
            if submitted {
                let _ = cx.update(|window, cx| {
                    let _ = this.update(cx, |this, cx| {
                        this.focus_active_workspace_surface(window, cx)
                    });
                });
            }
        })
        .detach();
        cx.notify();
    }

    pub(in crate::features) fn save_authenticated_credentials(
        &mut self,
        request_id: String,
        saves: Vec<PreparedCredentialSave>,
        attempt: nyaterm_transport::connection_attempt::ConnectionAttempt,
        cx: &mut Context<Self>,
    ) {
        let store = self.store_blocking_client();
        let scheduler = self.blocking_jobs.clone();
        cx.spawn(async move |this, cx| {
            let task = scheduler.submit_task("ssh-auth-save", move |_| {
                if attempt.is_cancelled() {
                    return Ok(None);
                }
                store
                    .request_mutation_fn(StoreDomain::Security, move |store| {
                        if attempt.is_cancelled() {
                            return Ok(None);
                        }
                        for save in saves {
                            store.save_runtime_credential(save)?;
                        }
                        store.load_sessions().map(Some)
                    })
                    .map_err(|e| e.to_string())
            });
            let result = await_blocking_result(task).await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(Some(sessions)) => {
                        this.apply_loaded_sessions(sessions, cx);
                        this.refresh_security_catalog(cx);
                        this.request_shared_state_refresh(
                            crate::app_shell::SharedStateDomain::Connections,
                            cx,
                        );
                        this.request_shared_state_refresh(
                            crate::app_shell::SharedStateDomain::Security,
                            cx,
                        );
                    }
                    Ok(None) => {}
                    Err(_) => {
                        let message = t!("sshAuth.saveFailed").to_string();
                        this.shell.set_status(message.clone());
                        this.security.set_status(message.clone());
                        this.notify_operation(
                            "ssh-auth-save",
                            nyaterm_ui::notification::NyaNotificationKind::Warning,
                            message,
                            cx,
                        );
                        tracing::warn!(
                            request_id,
                            failure_category = "storage",
                            "SSH authenticated, but runtime credential persistence failed"
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(in crate::features) fn change_runtime_auth_select(
        &mut self,
        id: &str,
        value: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        if !id.starts_with("ssh-auth-") {
            return false;
        }
        let Some(form) = self
            .session
            .prompts
            .active_credential_mut()
            .and_then(|s| s.authentication.as_mut())
        else {
            return true;
        };
        if form.busy {
            return true;
        }
        match id {
            "ssh-auth-password" => {
                form.password_id = Some(value.into());
                form.remember = false;
            }
            "ssh-auth-key" => {
                form.key_id = Some(value.into());
                form.remember = false;
            }
            "ssh-auth-save" => {
                form.destination = match value {
                    "new" => SaveDestination::NewAccount,
                    "linked" => SaveDestination::LinkedAccount,
                    _ => SaveDestination::Connection,
                }
            }
            _ => {}
        }
        form.error = None;
        cx.notify();
        true
    }

    pub(in crate::features) fn change_runtime_auth_source(
        &mut self,
        method: bool,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(state) = self.session.prompts.active_credential_mut()
            && let Some(form) = &mut state.authentication
        {
            if form.busy {
                return;
            }
            if method {
                form.change_source(
                    if index == 0 {
                        AuthMethod::Password
                    } else {
                        AuthMethod::Key
                    },
                    false,
                );
            } else {
                form.change_source(AuthMethod::Password, index == 1);
            }
            state.value.clear();
        }
        self.forget_text_inputs("ssh.credential.");
        self.forget_text_inputs("ssh.auth.");
        self.hide_secret_inputs(cx);
        self.session.prompt_restore_credential_focus();
        self.focus_active_ssh_prompt_input(window, cx);
        cx.notify();
    }

    pub(in crate::features) fn apply_runtime_auth_name(
        &mut self,
        name: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(form) = self
            .session
            .prompts
            .active_credential_mut()
            .and_then(|s| s.authentication.as_mut())
            && !form.busy
        {
            form.name = name;
        }
        cx.notify();
    }
}

fn prepare_response(
    store: &nyaterm_store::ConnectionStore,
    form: &RuntimeAuthenticationForm,
    secret: SecretString,
) -> Result<
    (
        SshAuthenticationResponse,
        Option<String>,
        Option<PreparedCredentialSave>,
    ),
    nyaterm_store::StorageError,
> {
    use nyaterm_store::StorageError;
    let invalid = || StorageError::InvalidData("SSH credential is unavailable".into());
    let mut key_id = None;
    let material = if form.is_passphrase() {
        SshAuthenticationMaterial::KeyPassphrase(secret)
    } else if form.method == AuthMethod::Key {
        let id = form.key_id.as_ref().ok_or_else(invalid)?;
        let key = store
            .load_decrypted_ssh_key_by_id(id)?
            .ok_or_else(invalid)?;
        key_id = Some(id.clone());
        SshAuthenticationMaterial::Key(SshKeyAuthConfig {
            key_data: key.key_data.filter(|k| !k.is_empty()).ok_or_else(invalid)?,
            cert_data: key.cert_data,
            passphrase: key.passphrase,
        })
    } else if form.saved_password {
        let account = store
            .load_decrypted_password_by_id(form.password_id.as_deref().ok_or_else(invalid)?)?
            .ok_or_else(invalid)?;
        SshAuthenticationMaterial::Password(
            account
                .password
                .filter(|p| !p.is_empty())
                .ok_or_else(invalid)?,
        )
    } else {
        SshAuthenticationMaterial::Password(secret)
    };
    let save = if form.remember && form.can_remember() {
        let (intent, secret) = if form.is_passphrase() {
            let key_id = form
                .context
                .selected_key(form.request.key_response_id.as_deref())
                .ok_or_else(invalid)?;
            let SshAuthenticationMaterial::KeyPassphrase(secret) = &material else {
                return Err(invalid());
            };
            let expected = form
                .context
                .selected_key_material(form.request.key_response_id.as_deref())
                .ok_or_else(invalid)?;
            let current = store
                .load_decrypted_ssh_key_by_id(&key_id)?
                .ok_or_else(invalid)?;
            if current.key_data.as_ref() != Some(&expected.key_data)
                || current.cert_data != expected.cert_data
                || current.passphrase != expected.passphrase
            {
                return Err(invalid());
            }
            (
                SshCredentialSaveIntent::KeyPassphrase { key_id },
                Some(secret.clone()),
            )
        } else if form.saved_password {
            (
                SshCredentialSaveIntent::AccountReference {
                    account_id: form.password_id.clone().ok_or_else(invalid)?,
                },
                None,
            )
        } else {
            let SshAuthenticationMaterial::Password(secret) = &material else {
                return Err(invalid());
            };
            let intent = match form.destination {
                SaveDestination::Connection => SshCredentialSaveIntent::ConnectionPassword,
                SaveDestination::NewAccount => SshCredentialSaveIntent::AccountPassword {
                    account_id: None,
                    name: if form.name.trim().is_empty() {
                        format!("{} SSH", form.request.prompt.connection_name)
                    } else {
                        form.name.clone()
                    },
                },
                SaveDestination::LinkedAccount => SshCredentialSaveIntent::AccountPassword {
                    account_id: Some(form.context.account_id.clone().ok_or_else(invalid)?),
                    name: String::new(),
                },
            };
            (intent, Some(secret.clone()))
        };
        Some(store.prepare_runtime_credential_save(
            form.context.connection.clone(),
            form.request.prompt.username.clone(),
            intent,
            secret,
        )?)
    } else {
        None
    };
    Ok((
        SshAuthenticationResponse {
            material,
            response_id: Some(nyaterm_core::uuid()),
        },
        key_id,
        save,
    ))
}

#[cfg(test)]
mod tests;
