use gpui::{App, Context, Modifiers, Subscription};
use nyaterm_core::SecretString;
use rust_i18n::t;

use super::ConnectionFeatureState;
use crate::features::NyaTermApp;
use crate::models::{
    ConnectionEditorField, ConnectionEditorPasswordSource, ConnectionPasswordEdit,
};

#[derive(Default)]
pub(super) struct EditorPasswordState {
    pub(super) has_saved: bool,
    revision: u64,
    pending: bool,
    revealed_account: Option<SecretString>,
    key_subscription: Option<Subscription>,
}

impl EditorPasswordState {
    pub(super) fn new(has_saved: bool) -> Self {
        Self {
            has_saved,
            ..Self::default()
        }
    }
}

impl ConnectionFeatureState {
    pub(super) fn bind_editor_password_keys(&mut self, cx: &mut Context<NyaTermApp>) {
        let app = cx.weak_entity();
        // Input actions consume deletion before ordinary KeyDown listeners.
        // Intercept only the focused, unloaded connection password, before keymap dispatch.
        self.editor.password.key_subscription =
            Some(cx.intercept_keystrokes(move |event, window, cx| {
                if !matches!(event.keystroke.key.as_str(), "backspace" | "delete")
                    || event.keystroke.modifiers != Modifiers::default()
                {
                    return;
                }
                let handled = app
                    .update(cx, |app, cx| {
                        if app.security.screen_locked()
                            || !app.connection_state.editor_password_can_delete_stored()
                        {
                            return false;
                        }
                        let Some(input) = app
                            .connection_state
                            .editor_fields()
                            .get(&ConnectionEditorField::Password)
                            .cloned()
                        else {
                            return false;
                        };
                        if !input.read(cx).component_focus_handle(cx).is_focused(window)
                            || input.update(cx, |state, cx| state.is_composing(window, cx))
                        {
                            return false;
                        }
                        let changed = app.connection_state.delete_editor_stored_password(cx);
                        if changed {
                            cx.notify();
                        }
                        changed
                    })
                    .unwrap_or(false);
                if handled {
                    cx.stop_propagation();
                }
            }));
    }

    pub fn invalidate_editor_password_request(&mut self) {
        self.editor.password.revision = self.editor.password.revision.wrapping_add(1);
        self.editor.password.pending = false;
        self.editor.password.revealed_account = None;
    }

    pub fn begin_editor_password_reveal(&mut self) -> Option<(u64, u64)> {
        self.editor.draft.as_ref()?;
        self.invalidate_editor_password_request();
        self.editor.password.pending = true;
        Some((self.editor.save_generation, self.editor.password.revision))
    }

    pub fn editor_password_request_matches(&self, token: (u64, u64)) -> bool {
        self.editor.draft.is_some()
            && self.editor.password.pending
            && token == (self.editor.save_generation, self.editor.password.revision)
    }

    pub fn editor_password_uses_account(&self) -> bool {
        self.editor.draft.as_ref().is_some_and(|draft| {
            draft.password_account_id().is_some()
                && draft.password_source != ConnectionEditorPasswordSource::Direct
        })
    }

    pub fn editor_password_placeholder(&self) -> &'static str {
        if self.editor_password_uses_account() {
            "authForm.connectionPasswordAccount"
        } else if self.editor.password.has_saved
            && self
                .editor
                .draft
                .as_ref()
                .is_some_and(|draft| draft.password_edit == ConnectionPasswordEdit::Keep)
        {
            "authForm.connectionPasswordStored"
        } else {
            "authForm.connectionPasswordEnter"
        }
    }

    pub fn sync_editor_password_input(&self, cx: &mut App) {
        let Some(draft) = self.editor.draft.as_ref() else {
            return;
        };
        let Some(input) = self.editor.fields.get(&ConnectionEditorField::Password) else {
            return;
        };
        let value = if self.editor_password_uses_account() {
            self.editor
                .password
                .revealed_account
                .as_deref()
                .unwrap_or("")
        } else {
            draft.password.expose_secret()
        };
        input.update(cx, |state, cx| {
            state.set_readonly(draft.password_account_id().is_some(), cx);
            state.set_placeholder(t!(self.editor_password_placeholder()), cx);
            if state.value(cx) != value {
                state.set_content_silent(value, cx);
            }
        });
    }

    /// A placeholder represents a saved secret without becoming editable text.
    pub fn editor_password_can_delete_stored(&self) -> bool {
        self.editor_password_needs_loading()
            && self
                .editor
                .draft
                .as_ref()
                .is_some_and(|draft| draft.password_account_id().is_none())
    }

    pub fn editor_password_needs_loading(&self) -> bool {
        self.editor.password.has_saved
            && self.editor.draft.as_ref().is_some_and(|draft| {
                draft.password_edit == ConnectionPasswordEdit::Keep && draft.password.is_empty()
            })
    }

    pub fn delete_editor_stored_password(&mut self, cx: &mut App) -> bool {
        if !self.editor_password_can_delete_stored() {
            return false;
        }
        self.set_editor_password_edit(ConnectionPasswordEdit::Remove, cx);
        self.sync_editor_password_input(cx);
        true
    }

    pub fn complete_editor_password_reveal(
        &mut self,
        token: (u64, u64),
        result: Result<Option<SecretString>, String>,
        cx: &mut App,
    ) {
        if !self.editor_password_request_matches(token) {
            return;
        }
        self.editor.password.pending = false;
        let value = match result {
            Ok(Some(value)) => value,
            Ok(None) | Err(_) => {
                self.set_editor_field_error(
                    ConnectionEditorField::Password,
                    t!("authForm.connectionPasswordUnavailable").to_string(),
                );
                return;
            }
        };
        if self.editor_password_uses_account() {
            self.editor.password.revealed_account = Some(value);
        } else if let Some(draft) = self.editor.draft.as_mut() {
            // Loading the original is presentation, not a replacement operation.
            if draft.password_edit == ConnectionPasswordEdit::Keep {
                draft.existing_password = Some(value.clone());
                draft.existing_password_locked = false;
                draft.password = value;
            }
        }
        self.sync_editor_password_input(cx);
        if let Some(input) = self.editor.fields.get(&ConnectionEditorField::Password) {
            input.update(cx, |state, cx| state.set_masked(false, cx));
        }
        if let Some(draft) = self.editor.draft.as_mut() {
            draft.error = None;
            draft.field_errors.remove(&ConnectionEditorField::Password);
        }
    }
}
