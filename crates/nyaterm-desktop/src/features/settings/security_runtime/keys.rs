use rust_i18n::t;

use gpui::{Context, IntoElement as _, KeyDownEvent, PathPromptOptions, SharedString, Window};
use nyaterm_core::SshKey;
use nyaterm_ui::NyaDialogWindowExt as _;

use crate::features::{NyaTermApp, formatting::compact_id, runtime_jobs::await_blocking_job};
use crate::models::{SecurityAuthTab, SecurityKeyEditorState};

use super::jobs::{SecurityStoreLocation, load_security_catalog};

impl NyaTermApp {
    pub(in crate::features) fn open_security_key_editor(
        &mut self,
        key_id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Editing an existing key reveals the same secrets as the private-key viewer.
        if let Some(id) = key_id.as_ref()
            && !self.require_security_secrets_unlocked(
                window,
                cx,
                Some(crate::models::SecurityUnlockAction::OpenKeyEditor(
                    id.clone(),
                )),
            )
        {
            return;
        }
        self.forget_text_inputs("security.editor.key-");
        let editor = if let Some(key_id) = key_id {
            let Some(key) = self
                .security
                .ssh_keys()
                .iter()
                .find(|key| key.id == key_id)
                .cloned()
            else {
                self.security.set_status("SSH key is no longer available");
                cx.notify();
                return;
            };
            SecurityKeyEditorState {
                id: Some(key.id),
                name: key.name,
                key_file_path: String::new(),
                key_data: nyaterm_core::SecretString::default(),
                cert_file_path: String::new(),
                cert_data: nyaterm_core::SecretString::default(),
                passphrase: nyaterm_core::SecretString::default(),
                key_content_mode: false,
                cert_content_mode: false,
                cert_expanded: key.has_cert_data,
                has_key_data: key.has_key_data,
                has_cert_data: key.has_cert_data,
                error: None,
            }
        } else {
            SecurityKeyEditorState {
                id: None,
                name: String::new(),
                key_file_path: String::new(),
                key_data: nyaterm_core::SecretString::default(),
                cert_file_path: String::new(),
                cert_data: nyaterm_core::SecretString::default(),
                passphrase: nyaterm_core::SecretString::default(),
                key_content_mode: true,
                cert_content_mode: false,
                cert_expanded: false,
                has_key_data: false,
                has_cert_data: false,
                error: None,
            }
        };
        self.security
            .open_key_editor(editor, "SSH key editor opened".to_string());
        window.focus(self.security.key_editor_focus(), cx);
        let title = if self
            .security
            .key_editor()
            .is_some_and(|editor| editor.id.is_some())
        {
            t!("securityAuth.editKeyTitle")
        } else {
            t!("securityAuth.newKeyTitle")
        }
        .to_string();
        let save = t!("common.save").to_string();
        self.open_guarded_form_dialog(
            (
                title,
                720.,
                save,
                |app, _, cx| {
                    app.security
                        .key_editor()
                        .cloned()
                        .map(|editor| app.security_key_editor_view(editor, cx).into_any_element())
                        .unwrap_or_else(|| gpui::div().into_any_element())
                },
                |app, window, cx| {
                    app.save_security_key_editor(window, cx);
                    app.security.key_editor().is_none()
                },
                |app, cx| app.close_security_key_editor(cx),
                |app| app.security.editor_busy(),
            ),
            window,
            cx,
        );
        if let Some(key_id) = self
            .security
            .key_editor()
            .and_then(|editor| editor.id.clone())
        {
            self.load_security_key_editor_secrets(key_id, cx);
        }
        cx.notify();
    }

    pub(in crate::features) fn close_security_key_editor(&mut self, cx: &mut Context<Self>) {
        if self.security.editor_busy() {
            return;
        }
        self.forget_text_inputs("security.editor.key-");
        self.security.close_key_editor();
        self.session.prompt_restore_credential_focus();
        cx.notify();
    }

    pub(in crate::features) fn toggle_security_key_content_mode(
        &mut self,
        is_cert: bool,
        content_mode: bool,
        cx: &mut Context<Self>,
    ) {
        if self.security.editor_busy() {
            return;
        }
        let reset_input = if let Some(editor) = self.security.key_editor_mut() {
            let reset_input = if is_cert {
                editor.cert_content_mode = content_mode;
                if content_mode {
                    editor.cert_file_path.clear();
                    "security.editor.key-cert-path"
                } else {
                    editor.cert_data.expose_secret_mut().clear();
                    "security.editor.key-cert-data"
                }
            } else {
                editor.key_content_mode = content_mode;
                if content_mode {
                    editor.key_file_path.clear();
                    "security.editor.key-path"
                } else {
                    editor.key_data.expose_secret_mut().clear();
                    "security.editor.key-data"
                }
            };
            editor.error = None;
            reset_input
        } else {
            return;
        };
        self.reset_text_input(reset_input, "", cx);
        cx.notify();
    }

    pub(in crate::features) fn toggle_security_key_certificate(&mut self, cx: &mut Context<Self>) {
        if self.security.editor_busy() {
            return;
        }
        if let Some(editor) = self.security.key_editor_mut() {
            editor.cert_expanded = !editor.cert_expanded;
        }
        cx.notify();
    }

    pub(in crate::features) fn handle_security_key_editor_key_down(
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
                    self.close_security_key_editor(cx);
                }
            }
            "enter" => {
                self.save_security_key_editor(window, cx);
            }
            _ => {}
        }
    }

    pub(in crate::features) fn save_security_key_editor(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.security.key_editor().cloned() else {
            return;
        };
        let name = editor.name.trim().to_string();
        if name.is_empty() {
            if let Some(editor) = self.security.key_editor_mut() {
                editor.error = Some("key name is required".to_string());
            }
            cx.notify();
            return;
        }
        if editor.id.is_none()
            && editor.key_file_path.trim().is_empty()
            && editor.key_data.trim().is_empty()
            && !editor.has_key_data
        {
            if let Some(editor) = self.security.key_editor_mut() {
                editor.error = Some("select a private key file".to_string());
            }
            cx.notify();
            return;
        }

        let key = SshKey {
            sort_order: 0,
            id: editor.id.clone().unwrap_or_default(),
            name,
            key: (!editor.key_data.trim().is_empty()).then_some(editor.key_data),
            cert: (!editor.cert_data.trim().is_empty()).then_some(editor.cert_data),
            passphrase: if editor.passphrase.trim().is_empty() {
                None
            } else {
                Some(editor.passphrase.clone())
            },
            key_file_path: if editor.key_file_path.trim().is_empty() {
                None
            } else {
                Some(editor.key_file_path.trim().to_string())
            },
            cert_file_path: if editor.cert_file_path.trim().is_empty() {
                None
            } else {
                Some(editor.cert_file_path.trim().to_string())
            },
            has_key_data: false,
            has_cert_data: false,
        };
        let Some(request_id) = self.security.begin_editor_request() else {
            return;
        };
        let location = SecurityStoreLocation::new(self.store_blocking_client());
        let scheduler = self.blocking_jobs.clone();
        cx.spawn_in(window, async move |this, cx| {
            let task = scheduler.submit_task("ssh-key-save", move |_| {
                let store = location.open()?;
                let id = store.save_ssh_key(key).map_err(|error| error.to_string())?;
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
                        this.security.replace_catalog_state(catalog);
                        this.request_shared_state_refresh(
                            crate::app_shell::SharedStateDomain::Security,
                            cx,
                        );
                        this.security
                            .finish_key_editor(format!("SSH key saved ({})", compact_id(&id)));
                        this.shell.set_status("SSH key saved".to_string());
                        this.session.prompt_restore_credential_focus();
                        close = true;
                    }
                    Err(error) => {
                        if let Some(editor) = this.security.key_editor_mut() {
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

    pub(in crate::features) fn request_delete_security_key(
        &mut self,
        key_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let label = self
            .security
            .ssh_keys()
            .iter()
            .find(|key| key.id == key_id)
            .map(|key| key.name.clone())
            .unwrap_or_else(|| key_id.clone());
        self.open_security_delete_dialog(SecurityAuthTab::Keys, key_id, label, window, cx);
        cx.notify();
    }

    pub(in crate::features) fn pick_security_key_file(
        &mut self,
        is_cert: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.security.editor_busy() {
            return;
        }
        let options = PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(SharedString::from(if is_cert {
                "Select certificate file"
            } else {
                "Select private key file"
            })),
        };
        let receiver = cx.prompt_for_paths(options);
        self.security.set_status(if is_cert {
            "selecting certificate file"
        } else {
            "selecting private key file"
        });
        cx.spawn(async move |this, cx| {
            let selected = match receiver.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                _ => None,
            };
            let _ = this.update(cx, |this, cx| {
                if let Some(path) = selected {
                    let path = path.display().to_string();
                    let input_id;
                    if let Some(editor) = this.security.key_editor_mut() {
                        if is_cert {
                            editor.cert_file_path = path.clone();
                            editor.cert_data.expose_secret_mut().clear();
                            editor.has_cert_data = true;
                            input_id = "security.editor.key-cert-path";
                        } else {
                            editor.key_file_path = path.clone();
                            editor.key_data.expose_secret_mut().clear();
                            editor.has_key_data = true;
                            input_id = "security.editor.key-path";
                        }
                        editor.error = None;
                        this.reset_text_input(input_id, &path, cx);
                        this.security.set_status("key file selected");
                    }
                } else {
                    this.security.set_status("key file selection cancelled");
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(in crate::features) fn view_security_private_key(
        &mut self,
        key_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.require_security_secrets_unlocked(
            window,
            cx,
            Some(crate::models::SecurityUnlockAction::ViewPrivateKey(
                key_id.clone(),
            )),
        ) {
            return;
        }
        let Some(name) = self
            .security
            .ssh_keys()
            .iter()
            .find(|key| key.id == key_id)
            .map(|key| key.name.clone())
        else {
            self.security.set_status("SSH key is no longer available");
            cx.notify();
            return;
        };
        let request_id = self.security.begin_private_key_view(name);
        self.open_content_dialog(
            t!("settings.privateKeyDialogTitle").to_string(),
            720.,
            |app, _, cx| app.security_private_key_view(cx).into_any_element(),
            |app, cx| app.close_security_private_key_view(cx),
            window,
            cx,
        );

        let location = SecurityStoreLocation::new(self.store_blocking_client());
        let scheduler = self.blocking_jobs.clone();
        cx.spawn(async move |this, cx| {
            let task = scheduler.submit_task("private-key-view", move |_| {
                let store = location.open()?;
                store
                    .load_decrypted_ssh_key_by_id(&key_id)
                    .map_err(|error| error.to_string())
            });
            let result = await_blocking_job(task).await.and_then(|result| result);
            let _ = this.update(cx, |this, cx| {
                let value = match result {
                    Ok(Some(key)) => key
                        .key_data
                        .filter(|value| !value.is_empty())
                        .map(nyaterm_core::SecretString::into_secret)
                        .ok_or_else(|| t!("settings.privateKeyEmpty").to_string()),
                    Ok(None) => Err(t!("settings.privateKeyEmpty").to_string()),
                    Err(error) => Err(format!("{}: {error}", t!("settings.privateKeyLoadFailed"))),
                };
                if this.security.finish_private_key_view(request_id, value) {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(in crate::features) fn copy_security_private_key(&mut self, cx: &mut Context<Self>) {
        if let Some((_, value, None)) = self.security.private_key_view()
            && !value.is_empty()
        {
            self.copy_text_with_feedback("copy-private-key", value.to_string(), cx);
            self.security.set_status(t!("common.copied").to_string());
            cx.notify();
        } else {
            self.copy_text_with_feedback("copy-private-key", String::new(), cx);
        }
    }

    pub(in crate::features) fn copy_security_public_key(
        &mut self,
        key_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.require_security_secrets_unlocked(
            window,
            cx,
            Some(crate::models::SecurityUnlockAction::CopyPublicKey(
                key_id.clone(),
            )),
        ) {
            return;
        }
        let request_id = self.security.begin_public_key_request(key_id.clone());
        let request_key_id = key_id.clone();
        let location = SecurityStoreLocation::new(self.store_blocking_client());
        let scheduler = self.blocking_jobs.clone();
        let origin_window = Some(window.window_handle());
        cx.spawn(async move |this, cx| {
            let task = scheduler.submit_task("ssh-public-key-copy", move |_| {
                let store = location.open()?;
                let key = store
                    .load_decrypted_ssh_key_by_id(&key_id)
                    .map_err(|error| error.to_string())?
                    .and_then(|key| key.key_data)
                    .ok_or_else(|| t!("settings.privateKeyEmpty").to_string())?;
                nyaterm_core::ssh_keys::derive_public_key_for_copy(key.expose_secret())
                    .map_err(|_| t!("securityAuth.publicKeyCopyFailed").to_string())
            });
            let result = await_blocking_job(task).await.and_then(|result| result);
            let _ = this.update(cx, |this, cx| {
                if !this
                    .security
                    .finish_public_key_request(request_id, &request_key_id)
                    || !this.security.secrets_unlocked()
                {
                    return;
                }
                match result {
                    Ok(public) => {
                        this.copy_text_with_feedback_at(
                            origin_window,
                            "copy-public-key",
                            public,
                            cx,
                        );
                        this.security.set_status(t!("common.copied").to_string());
                        this.shell.set_status(t!("common.copied").to_string());
                    }
                    Err(error) => {
                        this.security.set_status(error.clone());
                        this.shell.set_status(error);
                        this.notify_operation_at(
                            origin_window,
                            "copy-public-key",
                            nyaterm_ui::notification::NyaNotificationKind::Error,
                            rust_i18n::t!("common.copyFailed").to_string(),
                            cx,
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(in crate::features) fn close_security_private_key_view(&mut self, cx: &mut Context<Self>) {
        self.security.close_private_key_view();
        cx.notify();
    }

    fn load_security_key_editor_secrets(&mut self, key_id: String, cx: &mut Context<Self>) {
        let Some(request_id) = self.security.begin_editor_request() else {
            return;
        };
        let location = SecurityStoreLocation::new(self.store_blocking_client());
        let scheduler = self.blocking_jobs.clone();
        cx.spawn(async move |this, cx| {
            let task = scheduler.submit_task("ssh-key-editor-secret", move |_| {
                let store = location.open()?;
                store
                    .load_decrypted_ssh_key_by_id(&key_id)
                    .map_err(|error| error.to_string())
            });
            let result = await_blocking_job(task).await.and_then(|result| result);
            let _ = this.update(cx, |this, cx| {
                if !this.security.finish_editor_request(request_id) {
                    return;
                }
                let Some(editor) = this.security.key_editor_mut() else {
                    return;
                };
                match result {
                    Ok(Some(key)) => {
                        editor.key_data = key.key_data.unwrap_or_default();
                        editor.cert_data = key.cert_data.unwrap_or_default();
                        editor.passphrase = key.passphrase.unwrap_or_default();
                        editor.key_content_mode = true;
                        editor.cert_content_mode = true;
                    }
                    Ok(None) => editor.error = Some("SSH key not found".to_string()),
                    Err(error) => editor.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }
}
