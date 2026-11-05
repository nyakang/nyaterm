use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc};

use nyaterm_store::runtime_auth::{PreparedCredentialSave, RuntimeConnectionSnapshot};
use nyaterm_transport::SshCredentialPromptKind;
use nyaterm_transport::connection_attempt::ConnectionAttempt;
use nyaterm_transport::ssh_authentication::{
    SshAuthenticationMethod, SshAuthenticationRequest, SshAuthenticationResponse,
    SshAuthenticationStage,
};

#[derive(Debug, Default)]
pub(in crate::features) struct RuntimeAuthenticationContext {
    pub connection: Option<RuntimeConnectionSnapshot>,
    pub account_id: Option<String>,
    pub initial_key_id: Option<String>,
    pub initial_key_material: Option<nyaterm_transport::SshKeyAuthConfig>,
    pub attempt: ConnectionAttempt,
    selections: Mutex<PendingSelections>,
}

#[derive(Debug, Default)]
struct PendingSelections {
    entries: HashMap<String, PendingSelection>,
    finished: bool,
}

#[derive(Debug)]
struct PendingSelection {
    key_id: Option<String>,
    key_material: Option<nyaterm_transport::SshKeyAuthConfig>,
    save: Option<PreparedCredentialSave>,
}

impl RuntimeAuthenticationContext {
    pub fn new(
        connection: Option<RuntimeConnectionSnapshot>,
        account_id: Option<String>,
        initial_key_id: Option<String>,
        initial_key_material: Option<nyaterm_transport::SshKeyAuthConfig>,
        attempt: ConnectionAttempt,
    ) -> Self {
        Self {
            connection,
            account_id,
            initial_key_id,
            initial_key_material,
            attempt,
            selections: Mutex::default(),
        }
    }

    pub fn for_attempt(&self, attempt: ConnectionAttempt) -> Self {
        Self::new(
            self.connection.clone(),
            self.account_id.clone(),
            self.initial_key_id.clone(),
            self.initial_key_material.clone(),
            attempt,
        )
    }

    pub fn remember_selection(
        &self,
        id: String,
        key_id: Option<String>,
        key_material: Option<nyaterm_transport::SshKeyAuthConfig>,
        save: Option<PreparedCredentialSave>,
    ) -> Result<(), String> {
        self.attempt.check()?;
        let mut pending = self
            .selections
            .lock()
            .map_err(|_| "SSH selection state is poisoned")?;
        // Completion and late submissions serialize on this lock. Once finished,
        // a cancelled worker must not put decrypted material back into the map.
        if pending.finished || self.attempt.is_cancelled() {
            return Err("SSH authentication request has finished".into());
        }
        pending.entries.insert(
            id,
            PendingSelection {
                key_id,
                key_material,
                save,
            },
        );
        Ok(())
    }

    pub fn selected_key(&self, response_id: Option<&str>) -> Option<String> {
        match response_id {
            Some(id) => self.selections.lock().ok()?.entries.get(id)?.key_id.clone(),
            None => self.initial_key_id.clone(),
        }
    }

    pub fn selected_key_material(
        &self,
        response_id: Option<&str>,
    ) -> Option<nyaterm_transport::SshKeyAuthConfig> {
        match response_id {
            Some(id) => self
                .selections
                .lock()
                .ok()?
                .entries
                .get(id)?
                .key_material
                .clone(),
            None => self.initial_key_material.clone(),
        }
    }

    pub fn begin_request(&self, key_response_id: Option<&str>, accepted_response_ids: &[String]) {
        if let Ok(mut pending) = self.selections.lock() {
            pending.entries.retain(|id, _| {
                Some(id.as_str()) == key_response_id || accepted_response_ids.contains(id)
            });
        }
    }

    pub fn finish(&self, successful: Option<&[String]>) -> Vec<PreparedCredentialSave> {
        let Ok(mut pending) = self.selections.lock() else {
            return Vec::new();
        };
        pending.finished = true;
        let entries = std::mem::take(&mut pending.entries);
        if self.attempt.is_cancelled() {
            return Vec::new();
        }
        entries
            .into_iter()
            .filter_map(|(id, selection)| {
                successful
                    .filter(|ids| ids.contains(&id))
                    .and(selection.save)
            })
            .collect()
    }
}

#[derive(Debug, Clone)]
pub(in crate::features) enum CredentialResponseSender {
    Secret(mpsc::Sender<Option<String>>),
    Authentication(mpsc::Sender<Option<SshAuthenticationResponse>>),
}

impl From<mpsc::Sender<Option<String>>> for CredentialResponseSender {
    fn from(sender: mpsc::Sender<Option<String>>) -> Self {
        Self::Secret(sender)
    }
}

impl CredentialResponseSender {
    pub fn send(&self, value: Option<String>) -> Result<(), String> {
        match self {
            Self::Secret(sender) => sender.send(value).map_err(|_| "SSH prompt closed".into()),
            Self::Authentication(sender) => {
                if value.is_some() {
                    return Err("structured SSH response required".into());
                }
                sender.send(None).map_err(|_| "SSH prompt closed".into())
            }
        }
    }

    pub fn send_authentication(&self, response: SshAuthenticationResponse) -> Result<(), String> {
        match self {
            Self::Authentication(sender) => sender
                .send(Some(response))
                .map_err(|_| "SSH prompt closed".into()),
            Self::Secret(_) => Err("legacy SSH prompt cannot select authentication methods".into()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AuthMethod {
    Password,
    Key,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SaveDestination {
    Connection,
    NewAccount,
    LinkedAccount,
}

#[derive(Debug, Clone)]
pub(in crate::features) struct RuntimeAuthenticationForm {
    pub request: SshAuthenticationRequest,
    pub context: Arc<RuntimeAuthenticationContext>,
    pub(super) method: AuthMethod,
    pub(super) saved_password: bool,
    pub(super) password_id: Option<String>,
    pub(super) key_id: Option<String>,
    pub(super) remember: bool,
    pub(super) destination: SaveDestination,
    pub(super) name: String,
    pub(super) busy: bool,
    pub(super) error: Option<String>,
}

impl RuntimeAuthenticationForm {
    pub fn new(
        request: SshAuthenticationRequest,
        context: Arc<RuntimeAuthenticationContext>,
    ) -> Self {
        let method = if request.available_methods.iter().any(|m| {
            matches!(
                m,
                SshAuthenticationMethod::Password | SshAuthenticationMethod::KeyboardInteractive
            )
        }) {
            AuthMethod::Password
        } else {
            AuthMethod::Key
        };
        Self {
            request,
            context,
            method,
            saved_password: false,
            password_id: None,
            key_id: None,
            remember: false,
            destination: SaveDestination::Connection,
            name: String::new(),
            busy: false,
            error: None,
        }
    }

    pub fn is_passphrase(&self) -> bool {
        self.request.prompt.kind == SshCredentialPromptKind::KeyPassphrase
    }
    pub fn uses_secret_input(&self) -> bool {
        self.is_passphrase() || (self.method == AuthMethod::Password && !self.saved_password)
    }
    pub fn is_busy(&self) -> bool {
        self.busy
    }
    pub fn selection_focus_id(&self) -> &'static str {
        if self.method == AuthMethod::Key {
            "ssh-auth-key"
        } else {
            "ssh-auth-password"
        }
    }
    pub fn can_remember(&self) -> bool {
        if self.request.stage == SshAuthenticationStage::AdditionalFactor && !self.is_passphrase() {
            return false;
        }
        if self.is_passphrase() {
            self.context
                .selected_key(self.request.key_response_id.as_deref())
                .is_some()
        } else {
            self.method == AuthMethod::Password && self.context.connection.is_some()
        }
    }
    pub fn can_submit(&self, secret: &str) -> bool {
        !self.busy
            && !self.context.attempt.is_cancelled()
            && if self.uses_secret_input() {
                !secret.is_empty()
            } else if self.method == AuthMethod::Key {
                self.key_id.is_some()
            } else {
                self.password_id.is_some()
            }
    }

    pub(super) fn change_source(&mut self, method: AuthMethod, saved_password: bool) {
        self.method = method;
        self.saved_password = saved_password;
        self.password_id = None;
        self.key_id = None;
        self.remember = false;
        self.destination = SaveDestination::Connection;
        self.name.clear();
        self.error = None;
    }
}

#[cfg(test)]
mod tests;
