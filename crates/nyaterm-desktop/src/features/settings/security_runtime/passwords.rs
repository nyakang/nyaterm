use rust_i18n::t;

use gpui::{ClipboardItem, Context, IntoElement as _, KeyDownEvent, Window};
use nyaterm_core::SavedPassword;
use nyaterm_ui::NyaDialogWindowExt as _;

use crate::features::{NyaTermApp, formatting::compact_id, runtime_jobs::await_blocking_job};
use crate::models::{SecurityAuthTab, SecurityPasswordEditorState, SecurityUnlockAction};

use super::jobs::{SecurityStoreLocation, load_security_catalog};

impl NyaTermApp {
    pub(in crate::features) fn open_security_password_editor(
        &mut self,
        password_id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.forget_text_inputs("security.editor.pw-");
        if password_id.is_some()
            && !self.require_security_secrets_unlocked(
                window,
                cx,
                Some(SecurityUnlockAction::OpenPasswordEditor(
                    password_id.clone(),
                )),
            )
        {
            return;
        }
        let editor = if let Some(password_id) = password_id {
            let Some(entry) = self
                .security
                .passwords()
                .iter()
                .find(|entry| entry.id == password_id)
                .cloned()
            else {
                self.security.set_status("password is no longer available");
                cx.notify();
                return;
            };
            SecurityPasswordEditorState {
                id: Some(entry.id),
                name: entry.name,
                username: entry.username,
                password: nyaterm_core::SecretString::default(),
                has_password: entry.has_password,
                show_password: false,
                error: None,
            }
        } else {
            SecurityPasswordEditorState {
                id: None,
                name: String::new(),
                username: String::new(),
                password: nyaterm_core::SecretString::default(),
                has_password: false,
                show_password: false,
                error: None,
            }
        };
        self.security
            .open_password_editor(editor, "password editor opened".to_string());
        window.focus(self.security.password_editor_focus(), cx);
        let title = if self
            .security
            .password_editor()
            .is_some_and(|editor| editor.id.is_some())
        {
            t!("passwordManager.editTitle")
        } else {
            t!("passwordManager.newTitle")
        }
        .to_string();
        self.open_guarded_form_dialog(
            (
                title,
                320.,
                t!("common.save").to_string(),
                |app, _, cx| {
                    app.security
                        .password_editor()
                        .cloned()
                        .map(|editor| {
                            app.security_password_editor_view(editor, cx)
                                .into_any_element()
                        })
                        .unwrap_or_else(|| gpui::div().into_any_element())
                },
                |app, window, cx| {
                    app.save_security_password_editor(window, cx);
                    app.security.password_editor().is_none()
                },
                |app, cx| app.close_security_password_editor(cx),
                |app| app.security.editor_busy(),
            ),
            window,
            cx,
        );
        if let Some(password_id) = self
            .security
            .password_editor()
            .and_then(|editor| editor.id.clone())
        {
            self.load_security_password_editor_secret(password_id, cx);
        }
        cx.notify();
    }

    pub(in crate::features) fn close_security_password_editor(&mut self, cx: &mut Context<Self>) {
        if self.security.editor_busy() {
            return;
        }
        self.forget_text_inputs("security.editor.pw-");
        self.security.close_password_editor();
        cx.notify();
    }

    pub(in crate::features) fn handle_security_password_editor_key_down(
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
                    self.close_security_password_editor(cx);
                }
            }
            "enter" => {
                self.save_security_password_editor(window, cx);
            }
            _ => {}
        }
    }

    pub(in crate::features) fn save_security_password_editor(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.security.password_editor().cloned() else {
            return;
        };
        let name = editor.name.trim().to_string();
        if name.is_empty() {
            if let Some(editor) = self.security.password_editor_mut() {
                editor.error = Some(t!("passwordManager.nameRequired").to_string());
            }
            cx.notify();
            return;
        }
        if editor.id.is_none()
            && editor.password.trim().is_empty()
            && editor.username.trim().is_empty()
        {
            if let Some(editor) = self.security.password_editor_mut() {
                editor.error = Some(t!("passwordManager.credentialsRequired").to_string());
            }
            cx.notify();
            return;
        }
        let entry = SavedPassword {
            id: editor.id.clone().unwrap_or_default(),
            name,
            username: editor.username.trim().to_string(),
            password: if editor.password.trim().is_empty() {
                None
            } else {
                Some(editor.password.clone())
            },
            has_password: false,
        };
        let Some(request_id) = self.security.begin_editor_request() else {
            return;
        };
        let location = SecurityStoreLocation::new(self.store_blocking_client());
        let scheduler = self.blocking_jobs.clone();
        cx.spawn_in(window, async move |this, cx| {
            let task = scheduler.submit_task("saved-password-save", move |_| {
                let store = location.open()?;
                let id = store
                    .save_password(entry)
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
                        this.security.hide_revealed_password(&id);
                        this.security.replace_catalog_state(catalog);
                        this.request_shared_state_refresh(
                            crate::app_shell::SharedStateDomain::Security,
                            cx,
                        );
                        this.security.finish_password_editor(format!(
                            "password saved ({})",
                            compact_id(&id)
                        ));
                        this.shell.set_status("password saved".to_string());
                        close = true;
                    }
                    Err(error) => {
                        if let Some(editor) = this.security.password_editor_mut() {
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

    pub(in crate::features) fn request_delete_security_password(
        &mut self,
        password_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.require_security_secrets_unlocked(
            window,
            cx,
            Some(SecurityUnlockAction::DeletePassword(password_id.clone())),
        ) {
            return;
        }
        let label = self
            .security
            .passwords()
            .iter()
            .find(|entry| entry.id == password_id)
            .map(|entry| entry.name.clone())
            .unwrap_or_else(|| password_id.clone());
        self.open_security_delete_dialog(
            SecurityAuthTab::Passwords,
            password_id,
            label,
            window,
            cx,
        );
        cx.notify();
    }

    pub(in crate::features) fn reveal_security_password(
        &mut self,
        password_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Tauri PasswordManagementTab: eye toggles reveal; hide does not need unlock.
        if self.security.hide_revealed_password(&password_id) {
            self.security.set_status("password hidden");
            cx.notify();
            return;
        }
        if !self.require_security_secrets_unlocked(
            window,
            cx,
            Some(SecurityUnlockAction::RevealPassword(password_id.clone())),
        ) {
            return;
        }
        self.load_security_password(password_id, false, cx);
        cx.notify();
    }

    pub(in crate::features) fn copy_security_password(
        &mut self,
        password_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.require_security_secrets_unlocked(
            window,
            cx,
            Some(SecurityUnlockAction::CopyPassword(password_id.clone())),
        ) {
            return;
        }
        if let Some(value) = self
            .security
            .revealed_password(&password_id)
            .map(str::to_string)
        {
            if value.is_empty() {
                self.security.set_status("password has no secret");
            } else {
                cx.write_to_clipboard(ClipboardItem::new_string(value));
                self.security.set_status("password copied");
            }
            cx.notify();
            return;
        }
        self.load_security_password(password_id, true, cx);
        cx.notify();
    }

    pub(in crate::features) fn toggle_security_password_editor_visibility(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        if self.security.editor_busy() {
            return;
        }
        if let Some(editor) = self.security.password_editor_mut() {
            editor.show_password = !editor.show_password;
            cx.notify();
        }
    }

    fn load_security_password(&mut self, password_id: String, copy: bool, cx: &mut Context<Self>) {
        let request_password_id = password_id.clone();
        let request_id = self.security.begin_password_request(password_id.clone());
        let location = SecurityStoreLocation::new(self.store_blocking_client());
        let scheduler = self.blocking_jobs.clone();
        cx.spawn(async move |this, cx| {
            let task = scheduler.submit_task("saved-password-reveal", move |_| {
                let store = location.open()?;
                store
                    .load_decrypted_password_by_id(&password_id)
                    .map_err(|error| error.to_string())
            });
            let result = await_blocking_job(task).await.and_then(|result| result);
            let _ = this.update(cx, |this, cx| {
                if !this
                    .security
                    .finish_password_request(request_id, &request_password_id)
                {
                    return;
                }
                match result {
                    Ok(Some(entry)) => {
                        let value = entry.password.unwrap_or_default().into_secret();
                        if value.is_empty() {
                            this.security.set_status("password has no secret");
                        } else {
                            this.security
                                .reveal_password(request_password_id.clone(), value.clone());
                            if copy {
                                cx.write_to_clipboard(ClipboardItem::new_string(value));
                                this.security.set_status("password revealed and copied");
                            } else {
                                this.security.set_status("password revealed");
                            }
                        }
                    }
                    Ok(None) => this.security.set_status("password not found"),
                    Err(error) => this.security.set_status(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn load_security_password_editor_secret(
        &mut self,
        password_id: String,
        cx: &mut Context<Self>,
    ) {
        let Some(request_id) = self.security.begin_editor_request() else {
            return;
        };
        let location = SecurityStoreLocation::new(self.store_blocking_client());
        let scheduler = self.blocking_jobs.clone();
        cx.spawn(async move |this, cx| {
            let task = scheduler.submit_task("saved-password-editor-secret", move |_| {
                let store = location.open()?;
                store
                    .load_decrypted_password_by_id(&password_id)
                    .map_err(|error| error.to_string())
            });
            let result = await_blocking_job(task).await.and_then(|result| result);
            let _ = this.update(cx, |this, cx| {
                if !this.security.finish_editor_request(request_id) {
                    return;
                }
                let Some(editor) = this.security.password_editor_mut() else {
                    return;
                };
                match result {
                    Ok(Some(entry)) => editor.password = entry.password.unwrap_or_default(),
                    Ok(None) => editor.error = Some("password not found".to_string()),
                    Err(error) => editor.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }
}
