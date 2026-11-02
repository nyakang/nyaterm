use gpui::{Context, Window};
use nyaterm_store::{StorageError, StoreDomain, store_request};

use crate::features::NyaTermApp;
use crate::models::{ConnectionEditorField, ConnectionPasswordEdit, SecurityUnlockAction};

impl NyaTermApp {
    pub(in crate::features) fn toggle_connection_password_visibility(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(input) = self
            .connection_state
            .editor_fields()
            .get(&ConnectionEditorField::Password)
            .cloned()
        else {
            return;
        };
        if !input.read(cx).is_masked() {
            self.connection_state.invalidate_editor_password_request();
            input.update(cx, |state, cx| state.hide_secret(cx));
            self.connection_state.sync_editor_password_input(cx);
            cx.notify();
            return;
        }
        let Some((editor_generation, revision)) =
            self.connection_state.begin_editor_password_reveal()
        else {
            return;
        };
        if self.security_secrets_locked()
            && !self.require_security_secrets_unlocked(
                window,
                cx,
                Some(SecurityUnlockAction::RevealConnectionPassword {
                    editor_generation,
                    revision,
                }),
            )
        {
            return;
        }
        self.reveal_connection_password_after_unlock((editor_generation, revision), cx);
    }

    pub(in crate::features) fn reveal_connection_password_after_unlock(
        &mut self,
        token: (u64, u64),
        cx: &mut Context<Self>,
    ) {
        if self.security_secrets_locked()
            || !self.connection_state.editor_password_request_matches(token)
        {
            return;
        }
        let Some(draft) = self.connection_state.active_editor_draft() else {
            return;
        };
        let account_id = self
            .connection_state
            .editor_password_uses_account()
            .then(|| draft.password_account_id().map(str::to_owned))
            .flatten();
        // A new draft is already known; unlocking the eye must never replace it
        // with the original stored password.
        if account_id.is_none()
            && (draft.password_edit != ConnectionPasswordEdit::Keep
                || !draft.password.is_empty()
                || !self.connection_state.editor_password_needs_loading())
        {
            self.connection_state.complete_editor_password_reveal(
                token,
                Ok(Some(draft.password)),
                cx,
            );
            cx.notify();
            return;
        }
        let connection_id = draft.id;
        self.submit_store_request(
            0,
            store_request(StoreDomain::Security, move |store| {
                if let Some(id) = account_id {
                    return store.load_decrypted_password_by_id(&id).and_then(
                        |entry| match entry {
                            Some(entry) => Ok(Some(entry.password.unwrap_or_default())),
                            None => Err(StorageError::InvalidData("account missing".into())),
                        },
                    );
                }
                let Some(id) = connection_id else {
                    return Ok(None);
                };
                let auth = store
                    .get_connection(&id)?
                    .and_then(|connection| connection.auth);
                match auth {
                    Some(auth) if auth.has_password => Err(StorageError::InvalidData(
                        "connection password could not be decrypted".into(),
                    )),
                    Some(auth) => Ok(auth.password),
                    None => Ok(None),
                }
            }),
            move |this, event, cx| {
                if this.security_secrets_locked() {
                    return;
                }
                this.connection_state.complete_editor_password_reveal(
                    token,
                    event
                        .outcome
                        .map_err(|_| "password unavailable".to_string()),
                    cx,
                );
                cx.notify();
            },
            cx,
        );
    }
}
