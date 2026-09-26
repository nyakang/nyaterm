use rust_i18n::t;

use gpui::{ClipboardItem, Context, IntoElement as _, KeyDownEvent, Window};
use nyaterm_core::{SavedCredential, validate_prompt_regex};
use nyaterm_ui::NyaDialogWindowExt as _;

use crate::features::{
    NyaTermApp, formatting::compact_id, formatting::none_if_blank, runtime_jobs::await_blocking_job,
};
use crate::models::{SecurityAuthTab, SecurityCredentialEditorState, SecurityUnlockAction};

use super::jobs::{SecurityStoreLocation, load_security_catalog};

impl NyaTermApp {
    pub(in crate::features) fn open_security_credential_editor(
        &mut self,
        credential_id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.forget_text_inputs("security.editor.cred-");
        if !self.require_security_secrets_unlocked(
            window,
            cx,
            Some(SecurityUnlockAction::OpenCredentialEditor(
                credential_id.clone(),
            )),
        ) {
            return;
        }
        let editor = if let Some(credential_id) = credential_id {
            let Some(entry) = self
                .security
                .credentials()
                .iter()
                .find(|entry| entry.id == credential_id)
                .cloned()
            else {
                self.security
                    .set_status("credential is no longer available");
                cx.notify();
                return;
            };
            SecurityCredentialEditorState {
                id: Some(entry.id),
                name: entry.name,
                username: entry.username,
                password: nyaterm_core::SecretString::default(),
                username_prompt_regex: entry.username_prompt_regex.unwrap_or_default(),
                password_prompt_regex: entry.password_prompt_regex.unwrap_or_default(),
                enabled: entry.enabled,
                has_password: entry.has_password,
                show_password: false,
                error: None,
            }
        } else {
            SecurityCredentialEditorState {
                id: None,
                name: String::new(),
                username: String::new(),
                password: nyaterm_core::SecretString::default(),
                username_prompt_regex: String::new(),
                password_prompt_regex: String::new(),
                enabled: true,
                has_password: false,
                show_password: false,
                error: None,
            }
        };
        self.security
            .open_credential_editor(editor, "credential editor opened".to_string());
        window.focus(self.security.credential_editor_focus(), cx);
        let title = if self
            .security
            .credential_editor()
            .is_some_and(|editor| editor.id.is_some())
        {
            t!("credentialManager.editTitle")
        } else {
            t!("credentialManager.newTitle")
        }
        .to_string();
        self.open_guarded_form_dialog(
            (
                title,
                640.,
                t!("common.save").to_string(),
                |app, _, cx| {
                    app.security
                        .credential_editor()
                        .cloned()
                        .map(|editor| {
                            app.security_credential_editor_view(editor, cx)
                                .into_any_element()
                        })
                        .unwrap_or_else(|| gpui::div().into_any_element())
                },
                |app, window, cx| {
                    app.save_security_credential_editor(window, cx);
                    app.security.credential_editor().is_none()
                },
                |app, cx| app.close_security_credential_editor(cx),
                |app| app.security.editor_busy(),
            ),
            window,
            cx,
        );
        if let Some(credential_id) = self
            .security
            .credential_editor()
            .and_then(|editor| editor.id.clone())
        {
            self.load_security_credential_editor_secret(credential_id, cx);
        }
        cx.notify();
    }

    pub(in crate::features) fn close_security_credential_editor(&mut self, cx: &mut Context<Self>) {
        if self.security.editor_busy() {
            return;
        }
        self.forget_text_inputs("security.editor.cred-");
        self.security.close_credential_editor();
        cx.notify();
    }

    pub(in crate::features) fn toggle_security_credential_enabled(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        if self.security.editor_busy() {
            return;
        }
        if let Some(editor) = self.security.credential_editor_mut() {
            editor.enabled = !editor.enabled;
        }
        cx.notify();
    }

    pub(in crate::features) fn toggle_security_credential_editor_visibility(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        if self.security.editor_busy() {
            return;
        }
        if let Some(editor) = self.security.credential_editor_mut() {
            editor.show_password = !editor.show_password;
        }
        cx.notify();
    }

    pub(in crate::features) fn toggle_security_credential_list_enabled(
        &mut self,
        credential_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.require_security_secrets_unlocked(
            window,
            cx,
            Some(SecurityUnlockAction::ToggleCredentialEnabled(
                credential_id.clone(),
            )),
        ) {
            return;
        }
        let Some(entry) = self
            .security
            .credentials()
            .iter()
            .find(|entry| entry.id == credential_id)
            .cloned()
        else {
            self.security.set_status("credential not found");
            cx.notify();
            return;
        };
        let mut next = entry;
        next.enabled = !next.enabled;
        let location = SecurityStoreLocation::new(self.store_blocking_client());
        let request_id = self
            .security
            .begin_credential_request(credential_id.clone());
        let scheduler = self.blocking_jobs.clone();
        cx.spawn(async move |this, cx| {
            let task = scheduler.submit_task("credential-toggle", move |_| {
                let store = location.open()?;
                store
                    .save_credential(next.clone())
                    .map_err(|error| error.to_string())?;
                let catalog = load_security_catalog(&store)?;
                Ok::<_, String>((next.name, next.enabled, catalog))
            });
            let result = await_blocking_job(task).await.and_then(|result| result);
            let _ = this.update(cx, |this, cx| {
                if !this
                    .security
                    .finish_credential_request(request_id, &credential_id)
                {
                    return;
                }
                match result {
                    Ok((name, enabled, catalog)) => {
                        this.security.replace_catalog_state(catalog);
                        this.request_shared_state_refresh(
                            crate::app_shell::SharedStateDomain::Security,
                            cx,
                        );
                        this.security.set_status(format!(
                            "credential {name} {}",
                            if enabled { "enabled" } else { "disabled" }
                        ));
                    }
                    Err(error) => this.security.set_status(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(in crate::features) fn handle_security_credential_editor_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.mark_user_activity();
        let keystroke = &event.keystroke;
        if keystroke.modifiers.platform || keystroke.modifiers.alt || keystroke.modifiers.control {
            return;
        }
        // The boxes own the text; the editor owns the keys that close or save
        // it, which the boxes leave unconsumed.
        match keystroke.key.as_str() {
            "escape" => {
                if !self.security.editor_busy() {
                    self.close_security_credential_editor(cx);
                }
            }
            "enter" => {
                self.save_security_credential_editor(window, cx);
            }
            _ => {}
        }
    }

    pub(in crate::features) fn save_security_credential_editor(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.security.credential_editor().cloned() else {
            return;
        };
        let name = editor.name.trim().to_string();
        if name.is_empty() {
            if let Some(editor) = self.security.credential_editor_mut() {
                editor.error = Some("credential name is required".to_string());
            }
            cx.notify();
            return;
        }
        for (label, pattern) in [
            ("username", editor.username_prompt_regex.trim()),
            ("password", editor.password_prompt_regex.trim()),
        ] {
            if !pattern.is_empty() && !validate_prompt_regex(pattern) {
                if let Some(editor) = self.security.credential_editor_mut() {
                    editor.error = Some(format!("invalid {label} prompt regular expression"));
                }
                cx.notify();
                return;
            }
        }
        let entry = SavedCredential {
            id: editor.id.clone().unwrap_or_default(),
            sort_order: editor
                .id
                .as_deref()
                .and_then(|id| {
                    self.security
                        .credentials()
                        .iter()
                        .find(|entry| entry.id == id)
                        .map(|entry| entry.sort_order)
                })
                .unwrap_or_default(),
            name,
            username: editor.username.trim().to_string(),
            password: if editor.password.trim().is_empty() {
                None
            } else {
                Some(editor.password.clone())
            },
            username_prompt_regex: none_if_blank(&editor.username_prompt_regex),
            password_prompt_regex: none_if_blank(&editor.password_prompt_regex),
            enabled: editor.enabled,
            has_password: false,
        };
        let Some(request_id) = self.security.begin_editor_request() else {
            return;
        };
        let location = SecurityStoreLocation::new(self.store_blocking_client());
        let scheduler = self.blocking_jobs.clone();
        cx.spawn_in(window, async move |this, cx| {
            let task = scheduler.submit_task("credential-save", move |_| {
                let store = location.open()?;
                let id = store
                    .save_credential(entry)
                    .map_err(|error| error.to_string())?;
                let catalog = load_security_catalog(&store)?;
                Ok::<_, String>((id, catalog))
            });
            let result = await_blocking_job(task).await.and_then(|result| result);
            let mut close = false;
            let _ = this.update(cx, |this, cx| {
                if !this.security.finish_editor_request(request_id) {
                    return;
                }
                match result {
                    Ok((id, catalog)) => {
                        this.security.hide_revealed_credential(&id);
                        this.security.replace_catalog_state(catalog);
                        this.request_shared_state_refresh(
                            crate::app_shell::SharedStateDomain::Security,
                            cx,
                        );
                        this.security.finish_credential_editor(format!(
                            "credential saved ({})",
                            compact_id(&id)
                        ));
                        this.shell.set_status("credential saved".to_string());
                        close = true;
                    }
                    Err(error) => {
                        if let Some(editor) = this.security.credential_editor_mut() {
                            editor.error = Some(error);
                        }
                    }
                }
                cx.notify();
            });
            if close {
                let _ = cx.update(|window, cx| window.close_nya_dialog(cx));
            }
        })
        .detach();
        cx.notify();
    }

    pub(in crate::features) fn request_delete_security_credential(
        &mut self,
        credential_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.require_security_secrets_unlocked(
            window,
            cx,
            Some(SecurityUnlockAction::DeleteCredential(
                credential_id.clone(),
            )),
        ) {
            return;
        }
        let label = self
            .security
            .credentials()
            .iter()
            .find(|entry| entry.id == credential_id)
            .map(|entry| entry.name.clone())
            .unwrap_or_else(|| credential_id.clone());
        self.open_security_delete_dialog(
            SecurityAuthTab::Credentials,
            credential_id,
            label,
            window,
            cx,
        );
        cx.notify();
    }

    pub(in crate::features) fn reveal_security_credential_password(
        &mut self,
        credential_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.security.hide_revealed_credential(&credential_id) {
            self.security.set_status("credential password hidden");
            cx.notify();
            return;
        }
        if !self.require_security_secrets_unlocked(
            window,
            cx,
            Some(SecurityUnlockAction::RevealCredential(
                credential_id.clone(),
            )),
        ) {
            return;
        }
        let request_credential_id = credential_id.clone();
        let request_id = self
            .security
            .begin_credential_request(credential_id.clone());
        let location = SecurityStoreLocation::new(self.store_blocking_client());
        let scheduler = self.blocking_jobs.clone();
        cx.spawn(async move |this, cx| {
            let task = scheduler.submit_task("credential-reveal", move |_| {
                let store = location.open()?;
                store
                    .load_decrypted_credential_by_id(&credential_id)
                    .map_err(|error| error.to_string())
            });
            let result = await_blocking_job(task).await.and_then(|result| result);
            let _ = this.update(cx, |this, cx| {
                if !this
                    .security
                    .finish_credential_request(request_id, &request_credential_id)
                {
                    return;
                }
                match result {
                    Ok(Some(entry)) => {
                        let value = entry.password.unwrap_or_default().into_secret();
                        if value.is_empty() {
                            this.security.set_status("credential has no password");
                        } else {
                            this.security
                                .reveal_credential(request_credential_id.clone(), value.clone());
                            cx.write_to_clipboard(ClipboardItem::new_string(value));
                            this.security
                                .set_status("credential password revealed and copied");
                        }
                    }
                    Ok(None) => this.security.set_status("credential not found"),
                    Err(error) => this.security.set_status(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(in crate::features) fn copy_security_credential_username(
        &mut self,
        credential_id: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(username) = self
            .security
            .credentials()
            .iter()
            .find(|entry| entry.id == credential_id)
            .map(|entry| entry.username.clone())
            && !username.is_empty()
        {
            cx.write_to_clipboard(ClipboardItem::new_string(username));
            self.security.set_status(t!("common.copied").to_string());
            cx.notify();
        }
    }

    pub(in crate::features) fn copy_security_credential_password(
        &mut self,
        credential_id: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(value) = self.security.revealed_credential(&credential_id)
            && !value.is_empty()
        {
            cx.write_to_clipboard(ClipboardItem::new_string(value.to_string()));
            self.security.set_status(t!("common.copied").to_string());
            cx.notify();
        }
    }

    pub(in crate::features) fn reorder_security_credentials(
        &mut self,
        source_id: String,
        target_id: String,
        after: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(next) = self
            .security
            .reordered_credentials(&source_id, &target_id, after)
        else {
            return;
        };
        let updates = next
            .iter()
            .map(|entry| (entry.id.clone(), entry.sort_order))
            .collect::<Vec<_>>();
        let location = SecurityStoreLocation::new(self.store_blocking_client());
        let request_id = self.security.begin_reorder_request();
        let scheduler = self.blocking_jobs.clone();
        cx.spawn(async move |this, cx| {
            let task = scheduler.submit_task("credential-reorder", move |_| {
                let store = location.open()?;
                store
                    .reorder_credentials(&updates)
                    .map_err(|error| error.to_string())?;
                load_security_catalog(&store)
            });
            let result = await_blocking_job(task).await.and_then(|result| result);
            let _ = this.update(cx, |this, cx| {
                if !this.security.finish_reorder_request(request_id) {
                    return;
                }
                this.security.set_credential_drop_target(None);
                let status = match result {
                    Ok(catalog) => {
                        this.security.replace_catalog_state(catalog);
                        this.request_shared_state_refresh(
                            crate::app_shell::SharedStateDomain::Security,
                            cx,
                        );
                        t!("credentialManager.reorderSuccess").to_string()
                    }
                    Err(error) => {
                        format!("{}: {error}", t!("credentialManager.reorderFailed"))
                    }
                };
                this.security.set_status(status);
                cx.notify();
            });
        })
        .detach();
    }

    fn load_security_credential_editor_secret(
        &mut self,
        credential_id: String,
        cx: &mut Context<Self>,
    ) {
        let Some(request_id) = self.security.begin_editor_request() else {
            return;
        };
        let location = SecurityStoreLocation::new(self.store_blocking_client());
        let scheduler = self.blocking_jobs.clone();
        cx.spawn(async move |this, cx| {
            let task = scheduler.submit_task("credential-editor-secret", move |_| {
                let store = location.open()?;
                store
                    .load_decrypted_credential_by_id(&credential_id)
                    .map_err(|error| error.to_string())
            });
            let result = await_blocking_job(task).await.and_then(|result| result);
            let _ = this.update(cx, |this, cx| {
                if !this.security.finish_editor_request(request_id) {
                    return;
                }
                let Some(editor) = this.security.credential_editor_mut() else {
                    return;
                };
                match result {
                    Ok(Some(entry)) => editor.password = entry.password.unwrap_or_default(),
                    Ok(None) => editor.error = Some("credential not found".to_string()),
                    Err(error) => editor.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }
}
