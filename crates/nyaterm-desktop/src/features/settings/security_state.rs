//! Grouped security panel state.
//!
//! Only UI state lives here. Keys, passwords, credentials and OTP entries stay
//! in `nyaterm-core`; the maps below hold values the user has explicitly
//! revealed or codes generated for display, and they are cleared through the
//! same paths as before.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use gpui::FocusHandle;
use nyaterm_core::{OtpEntry, SavedCredential, SavedPassword, SecretString, SshKey};
use nyaterm_store::KnownHostEntry;

use crate::models::{
    SecurityAuthTab, SecurityCredentialDropTarget, SecurityCredentialEditorState,
    SecurityKeyEditorState, SecurityOtpEditorState, SecurityPasswordEditorState,
    SecurityUnlockAction,
};

pub(in crate::features) struct SecurityFeatureState {
    catalog: SecurityCatalogState,
    auth_tab: SecurityAuthTab,
    editors: SecurityEditorState,
    revealed: SecurityRevealedState,
    panel: SecurityPanelInteractionState,
    status: String,
    unlock: SecurityUnlockState,
    screen_lock: SecurityScreenLockState,
    known_hosts: Vec<KnownHostEntry>,
    known_hosts_generation: u64,
    known_hosts_loading: bool,
    known_hosts_pending: Option<(u64, Option<String>)>,
}

/// Persisted secret-adjacent catalogs loaded through `ConnectionStore`.
///
/// This type deliberately has no `Debug` implementation so callers cannot
/// accidentally log secret-bearing entries through the feature state.
pub(in crate::features) struct SecurityCatalogState {
    ssh_keys: Vec<SshKey>,
    otp_entries: Vec<OtpEntry>,
    passwords: Vec<SavedPassword>,
    credentials: Vec<SavedCredential>,
}

impl SecurityCatalogState {
    pub(in crate::features) fn new(
        ssh_keys: Vec<SshKey>,
        otp_entries: Vec<OtpEntry>,
        passwords: Vec<SavedPassword>,
        credentials: Vec<SavedCredential>,
    ) -> Self {
        Self {
            ssh_keys,
            otp_entries,
            passwords,
            credentials,
        }
    }
}

/// Focus handles the security panel needs at construction time.
pub(in crate::features) struct SecurityFeatureFocus {
    pub key_editor: FocusHandle,
    pub otp_editor: FocusHandle,
    pub password_editor: FocusHandle,
    pub credential_editor: FocusHandle,
    pub unlock: FocusHandle,
    pub screen_lock: FocusHandle,
}

/// The four security editors, each an optional draft plus its focus handle.
struct SecurityEditorState {
    key: Option<SecurityKeyEditorState>,
    key_focus: FocusHandle,
    otp: Option<SecurityOtpEditorState>,
    otp_focus: FocusHandle,
    otp_qr_importing: bool,
    otp_qr_request_id: u64,
    password: Option<SecurityPasswordEditorState>,
    password_focus: FocusHandle,
    credential: Option<SecurityCredentialEditorState>,
    credential_focus: FocusHandle,
    request_id: u64,
    busy: bool,
}

/// Values the user has explicitly revealed, plus generated OTP codes.
struct SecurityRevealedState {
    otp_codes: HashMap<String, SecretString>,
    passwords: HashMap<String, SecretString>,
    credentials: HashMap<String, SecretString>,
    private_key: Option<SecurityPrivateKeyViewState>,
    private_key_request_id: u64,
}

struct SecurityPrivateKeyViewState {
    name: String,
    value: SecretString,
    error: Option<String>,
    request_id: u64,
}

struct SecurityPanelInteractionState {
    visible_otp_ids: HashSet<String>,
    otp_refresh_armed: bool,
    credential_drop_target: Option<SecurityCredentialDropTarget>,
    request_id: u64,
    pending_request: Option<SecurityPanelRequest>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SecurityPanelRequestKind {
    Otp,
    Password,
    Credential,
    Delete,
    Reorder,
    PublicKey,
}

struct SecurityPanelRequest {
    id: u64,
    kind: SecurityPanelRequestKind,
    item_id: String,
}

/// Master password unlock prompt.
struct SecurityUnlockState {
    secrets_unlocked: bool,
    prompt_open: bool,
    master_required_prompt_open: bool,
    draft: SecretString,
    error: Option<String>,
    pending_action: Option<SecurityUnlockAction>,
    request_id: u64,
    busy: bool,
    focus: FocusHandle,
}

/// Whole-application idle/manual lock screen.
///
/// This is distinct from `SecurityUnlockState`, which gates access to stored
/// secrets while the rest of the application remains usable.
struct SecurityScreenLockState {
    locked: bool,
    restore_focus: Option<FocusHandle>,
    password_draft: SecretString,
    status: String,
    focus: FocusHandle,
    last_user_activity_at: Instant,
}

impl SecurityFeatureState {
    pub(in crate::features) fn new(
        catalog: SecurityCatalogState,
        secrets_unlocked: bool,
        status: String,
        focus: SecurityFeatureFocus,
    ) -> Self {
        Self {
            catalog,
            auth_tab: SecurityAuthTab::Keys,
            editors: SecurityEditorState {
                key: None,
                key_focus: focus.key_editor,
                otp: None,
                otp_focus: focus.otp_editor,
                otp_qr_importing: false,
                otp_qr_request_id: 0,
                password: None,
                password_focus: focus.password_editor,
                credential: None,
                credential_focus: focus.credential_editor,
                request_id: 0,
                busy: false,
            },
            revealed: SecurityRevealedState {
                otp_codes: HashMap::new(),
                passwords: HashMap::new(),
                credentials: HashMap::new(),
                private_key: None,
                private_key_request_id: 0,
            },
            panel: SecurityPanelInteractionState {
                visible_otp_ids: HashSet::new(),
                otp_refresh_armed: false,
                credential_drop_target: None,
                request_id: 0,
                pending_request: None,
            },
            status,
            unlock: SecurityUnlockState {
                secrets_unlocked,
                prompt_open: false,
                master_required_prompt_open: false,
                draft: SecretString::default(),
                error: None,
                pending_action: None,
                request_id: 0,
                busy: false,
                focus: focus.unlock,
            },
            screen_lock: SecurityScreenLockState {
                locked: false,
                restore_focus: None,
                password_draft: SecretString::default(),
                status: String::new(),
                focus: focus.screen_lock,
                last_user_activity_at: Instant::now(),
            },
            known_hosts: Vec::new(),
            known_hosts_generation: 0,
            known_hosts_loading: false,
            known_hosts_pending: None,
        }
    }

    pub(in crate::features) fn known_hosts(&self) -> &[KnownHostEntry] {
        &self.known_hosts
    }

    pub(in crate::features) fn known_hosts_loading(&self) -> bool {
        self.known_hosts_loading
    }

    pub(in crate::features) fn begin_known_hosts_load(&mut self) -> u64 {
        self.known_hosts_generation = self.known_hosts_generation.wrapping_add(1).max(1);
        self.known_hosts_loading = true;
        self.known_hosts_generation
    }

    pub(in crate::features) fn apply_known_hosts_load(
        &mut self,
        generation: u64,
        entries: Vec<KnownHostEntry>,
    ) -> bool {
        if generation != self.known_hosts_generation {
            return false;
        }
        self.known_hosts_loading = false;
        self.known_hosts = entries;
        true
    }

    pub(in crate::features) fn fail_known_hosts_load(
        &mut self,
        generation: u64,
        error: String,
    ) -> bool {
        if generation != self.known_hosts_generation {
            return false;
        }
        self.known_hosts_loading = false;
        self.status = error;
        true
    }

    pub(in crate::features) fn ssh_keys(&self) -> &[SshKey] {
        &self.catalog.ssh_keys
    }

    pub(in crate::features) fn otp_entries(&self) -> &[OtpEntry] {
        &self.catalog.otp_entries
    }

    pub(in crate::features) fn passwords(&self) -> &[SavedPassword] {
        &self.catalog.passwords
    }

    pub(in crate::features) fn credentials(&self) -> &[SavedCredential] {
        &self.catalog.credentials
    }

    pub(in crate::features) fn replace_catalog(
        &mut self,
        ssh_keys: Vec<SshKey>,
        otp_entries: Vec<OtpEntry>,
        passwords: Vec<SavedPassword>,
        credentials: Vec<SavedCredential>,
    ) {
        self.catalog = SecurityCatalogState::new(ssh_keys, otp_entries, passwords, credentials);
    }

    pub(in crate::features) fn replace_catalog_state(&mut self, catalog: SecurityCatalogState) {
        self.catalog = catalog;
    }

    pub(in crate::features) fn auth_tab(&self) -> SecurityAuthTab {
        self.auth_tab
    }

    pub(in crate::features) fn set_auth_tab(&mut self, tab: SecurityAuthTab) {
        self.auth_tab = tab;
        self.revealed.passwords.clear();
        self.revealed.credentials.clear();
        self.revealed.otp_codes.clear();
        self.revealed.private_key = None;
        self.panel.visible_otp_ids.clear();
        self.panel.otp_refresh_armed = false;
        self.panel.credential_drop_target = None;
        self.panel.pending_request = None;
        self.clear_editors();
        self.status = format!("{} tab", tab.label().to_lowercase());
    }

    pub(in crate::features) fn set_status(&mut self, status: impl Into<String>) {
        self.status = status.into();
    }

    pub(in crate::features) fn secrets_unlocked(&self) -> bool {
        self.unlock.secrets_unlocked
    }

    pub(in crate::features) fn unlock_prompt_open(&self) -> bool {
        self.unlock.prompt_open
    }

    pub(in crate::features) fn master_required_prompt_open(&self) -> bool {
        self.unlock.master_required_prompt_open
    }

    pub(in crate::features) fn unlock_draft(&self) -> &str {
        self.unlock.draft.expose_secret()
    }

    pub(in crate::features) fn unlock_error(&self) -> Option<&str> {
        self.unlock.error.as_deref()
    }

    pub(in crate::features) fn unlock_focus(&self) -> &FocusHandle {
        &self.unlock.focus
    }

    pub(in crate::features) fn set_pending_unlock_action(
        &mut self,
        action: Option<SecurityUnlockAction>,
    ) {
        self.unlock.pending_action = action;
    }

    pub(in crate::features) fn show_master_required_prompt(&mut self) {
        self.unlock.pending_action = None;
        self.unlock.prompt_open = false;
        self.unlock.master_required_prompt_open = true;
        self.unlock.draft.expose_secret_mut().clear();
        self.unlock.error = None;
        self.status = "master password required".to_string();
    }

    pub(in crate::features) fn show_unlock_prompt(&mut self) {
        self.unlock.master_required_prompt_open = false;
        self.unlock.prompt_open = true;
        self.unlock.draft.expose_secret_mut().clear();
        self.unlock.error = None;
        self.status = "enter master password to unlock secrets".to_string();
    }

    pub(in crate::features) fn cancel_unlock_prompt(&mut self) {
        self.unlock.pending_action = None;
        self.close_unlock_prompt();
    }

    pub(in crate::features) fn close_master_required_prompt(&mut self) {
        self.unlock.master_required_prompt_open = false;
        self.unlock.pending_action = None;
    }

    pub(in crate::features) fn complete_unlock(&mut self) -> Option<SecurityUnlockAction> {
        let pending_action = self.unlock.pending_action.take();
        self.unlock.secrets_unlocked = true;
        self.status = "secrets unlocked".to_string();
        self.close_unlock_prompt();
        pending_action
    }

    pub(in crate::features) fn reject_unlock(&mut self, error: String, status: &'static str) {
        self.unlock.draft.expose_secret_mut().clear();
        self.unlock.error = Some(error);
        self.status = status.to_string();
    }

    pub(in crate::features) fn apply_unlock_input(&mut self, text: String) {
        if self.unlock.busy {
            return;
        }
        self.unlock.draft = text.into();
        self.unlock.error = None;
    }

    pub(in crate::features) fn begin_unlock_request(&mut self) -> Option<(u64, SecretString)> {
        if self.unlock.busy || !self.unlock.prompt_open {
            return None;
        }
        self.unlock.request_id = self.unlock.request_id.wrapping_add(1).max(1);
        self.unlock.busy = true;
        self.unlock.error = None;
        Some((self.unlock.request_id, self.unlock.draft.clone()))
    }

    pub(in crate::features) fn finish_unlock_request(&mut self, request_id: u64) -> bool {
        if !self.unlock.busy || self.unlock.request_id != request_id {
            return false;
        }
        self.unlock.busy = false;
        true
    }

    pub(in crate::features) fn screen_locked(&self) -> bool {
        self.screen_lock.locked
    }

    pub(in crate::features) fn screen_lock_password_draft(&self) -> &str {
        self.screen_lock.password_draft.expose_secret()
    }

    pub(in crate::features) fn screen_lock_status(&self) -> &str {
        &self.screen_lock.status
    }

    pub(in crate::features) fn screen_lock_focus(&self) -> &FocusHandle {
        &self.screen_lock.focus
    }

    pub(in crate::features) fn screen_lock_idle_for(&self) -> Duration {
        self.screen_lock.last_user_activity_at.elapsed()
    }

    pub(in crate::features) fn activate_screen_lock(&mut self, status: String) {
        self.screen_lock.locked = true;
        self.screen_lock.password_draft.expose_secret_mut().clear();
        self.screen_lock.status = status;
    }

    pub(in crate::features) fn remember_screen_lock_focus(&mut self, focus: Option<FocusHandle>) {
        if !self.screen_lock.locked {
            self.screen_lock.restore_focus = focus;
        }
    }

    pub(in crate::features) fn take_screen_lock_restore_focus(&mut self) -> Option<FocusHandle> {
        self.screen_lock.restore_focus.take()
    }

    pub(in crate::features) fn deactivate_screen_lock(&mut self) {
        self.screen_lock.locked = false;
        self.screen_lock.password_draft.expose_secret_mut().clear();
        self.screen_lock.status.clear();
        self.screen_lock.last_user_activity_at = Instant::now();
    }

    pub(in crate::features) fn record_screen_lock_user_activity(&mut self) {
        if !self.screen_lock.locked {
            self.reset_screen_lock_idle_timer();
        }
    }

    pub(in crate::features) fn reset_screen_lock_idle_timer(&mut self) {
        self.screen_lock.last_user_activity_at = Instant::now();
    }

    pub(in crate::features) fn set_screen_lock_password_draft(
        &mut self,
        text: String,
        status: String,
    ) {
        self.screen_lock.password_draft = text.into();
        self.screen_lock.status = status;
    }

    pub(in crate::features) fn clear_screen_lock_password_with_status(&mut self, status: String) {
        self.screen_lock.password_draft.expose_secret_mut().clear();
        self.screen_lock.status = status;
    }

    pub(in crate::features) fn revealed_password(&self, id: &str) -> Option<&str> {
        self.revealed
            .passwords
            .get(id)
            .map(SecretString::expose_secret)
    }

    pub(in crate::features) fn hide_revealed_password(&mut self, id: &str) -> bool {
        self.revealed.passwords.remove(id).is_some()
    }

    pub(in crate::features) fn reveal_password(&mut self, id: String, value: String) {
        self.revealed.passwords.insert(id, value.into());
    }

    pub(in crate::features) fn revealed_credential(&self, id: &str) -> Option<&str> {
        self.revealed
            .credentials
            .get(id)
            .map(SecretString::expose_secret)
    }

    pub(in crate::features) fn hide_revealed_credential(&mut self, id: &str) -> bool {
        self.revealed.credentials.remove(id).is_some()
    }

    pub(in crate::features) fn reveal_credential(&mut self, id: String, value: String) {
        self.revealed.credentials.insert(id, value.into());
    }

    pub(in crate::features) fn revealed_otp_code(&self, id: &str) -> Option<&str> {
        self.revealed
            .otp_codes
            .get(id)
            .map(SecretString::expose_secret)
    }

    pub(in crate::features) fn clear_revealed_otp_code(&mut self, id: &str) {
        self.revealed.otp_codes.remove(id);
    }

    pub(in crate::features) fn reveal_otp_code(&mut self, id: String, code: String) {
        self.revealed.otp_codes.insert(id, code.into());
    }

    pub(in crate::features) fn otp_code_visible(&self, id: &str) -> bool {
        self.panel.visible_otp_ids.contains(id)
    }

    pub(in crate::features) fn toggle_otp_code_visible(&mut self, id: String) -> bool {
        if self.panel.visible_otp_ids.remove(&id) {
            self.revealed.otp_codes.remove(&id);
            return false;
        }
        self.panel.visible_otp_ids.insert(id);
        true
    }

    pub(in crate::features) fn visible_totp_ids(&self) -> Vec<String> {
        self.catalog
            .otp_entries
            .iter()
            .filter(|entry| {
                entry.otp_type.eq_ignore_ascii_case("totp")
                    && self.panel.visible_otp_ids.contains(&entry.id)
            })
            .map(|entry| entry.id.clone())
            .collect()
    }

    pub(in crate::features) fn arm_otp_refresh(&mut self) -> bool {
        if self.panel.otp_refresh_armed {
            return false;
        }
        self.panel.otp_refresh_armed = true;
        true
    }

    pub(in crate::features) fn disarm_otp_refresh(&mut self) {
        self.panel.otp_refresh_armed = false;
    }

    pub(in crate::features) fn private_key_view(&self) -> Option<(&str, &str, Option<&str>)> {
        self.revealed.private_key.as_ref().map(|view| {
            (
                view.name.as_str(),
                view.value.expose_secret(),
                view.error.as_deref(),
            )
        })
    }

    pub(in crate::features) fn begin_private_key_view(&mut self, name: String) -> u64 {
        self.revealed.private_key_request_id =
            self.revealed.private_key_request_id.wrapping_add(1).max(1);
        let request_id = self.revealed.private_key_request_id;
        self.revealed.private_key = Some(SecurityPrivateKeyViewState {
            name,
            value: SecretString::default(),
            error: None,
            request_id,
        });
        request_id
    }

    pub(in crate::features) fn finish_private_key_view(
        &mut self,
        request_id: u64,
        value: Result<String, String>,
    ) -> bool {
        let Some(view) = self.revealed.private_key.as_mut() else {
            return false;
        };
        if view.request_id != request_id {
            return false;
        }
        match value {
            Ok(value) => {
                view.value = value.into();
                view.error = None;
            }
            Err(error) => {
                view.value.expose_secret_mut().clear();
                view.error = Some(error);
            }
        }
        true
    }

    pub(in crate::features) fn close_private_key_view(&mut self) {
        self.revealed.private_key = None;
    }

    pub(in crate::features) fn credential_drop_target(
        &self,
    ) -> Option<&SecurityCredentialDropTarget> {
        self.panel.credential_drop_target.as_ref()
    }

    pub(in crate::features) fn editor_busy(&self) -> bool {
        self.editors.busy
    }

    pub(in crate::features) fn begin_editor_request(&mut self) -> Option<u64> {
        if self.editors.busy
            || (self.editors.key.is_none()
                && self.editors.otp.is_none()
                && self.editors.password.is_none()
                && self.editors.credential.is_none())
        {
            return None;
        }
        self.editors.request_id = self.editors.request_id.wrapping_add(1).max(1);
        self.editors.busy = true;
        Some(self.editors.request_id)
    }

    pub(in crate::features) fn finish_editor_request(&mut self, request_id: u64) -> bool {
        if !self.editors.busy || self.editors.request_id != request_id {
            return false;
        }
        self.editors.busy = false;
        true
    }

    pub(in crate::features) fn begin_password_request(&mut self, item_id: String) -> u64 {
        self.begin_panel_request(SecurityPanelRequestKind::Password, item_id)
    }

    pub(in crate::features) fn begin_otp_request(&mut self, item_id: String) -> u64 {
        self.begin_panel_request(SecurityPanelRequestKind::Otp, item_id)
    }

    pub(in crate::features) fn begin_credential_request(&mut self, item_id: String) -> u64 {
        self.begin_panel_request(SecurityPanelRequestKind::Credential, item_id)
    }

    pub(in crate::features) fn begin_delete_request(&mut self, item_id: String) -> u64 {
        self.begin_panel_request(SecurityPanelRequestKind::Delete, item_id)
    }

    pub(in crate::features) fn begin_reorder_request(&mut self) -> u64 {
        self.begin_panel_request(SecurityPanelRequestKind::Reorder, String::new())
    }

    pub(in crate::features) fn begin_known_host_delete(&mut self, item_id: String) -> Option<u64> {
        self.begin_known_hosts_mutation(Some(item_id))
    }

    pub(in crate::features) fn finish_known_host_delete(
        &mut self,
        request_id: u64,
        item_id: &str,
    ) -> bool {
        self.finish_known_hosts_mutation(request_id, Some(item_id))
    }

    pub(in crate::features) fn begin_known_hosts_clear(&mut self) -> Option<u64> {
        self.begin_known_hosts_mutation(None)
    }

    pub(in crate::features) fn finish_known_hosts_clear(&mut self, request_id: u64) -> bool {
        self.finish_known_hosts_mutation(request_id, None)
    }

    pub(in crate::features) fn known_hosts_busy(&self) -> bool {
        self.known_hosts_pending.is_some()
    }

    fn begin_known_hosts_mutation(&mut self, item: Option<String>) -> Option<u64> {
        if self.known_hosts_busy() || self.known_hosts_loading {
            return None;
        }
        // Invalidate any older read before the write is submitted.
        self.known_hosts_generation = self.known_hosts_generation.wrapping_add(1).max(1);
        let request = self.known_hosts_generation;
        self.known_hosts_pending = Some((request, item));
        Some(request)
    }

    fn finish_known_hosts_mutation(&mut self, request: u64, item: Option<&str>) -> bool {
        let matches = self
            .known_hosts_pending
            .as_ref()
            .is_some_and(|(id, pending)| *id == request && pending.as_deref() == item);
        if matches {
            self.known_hosts_pending = None;
        }
        matches
    }

    pub(in crate::features) fn begin_public_key_request(&mut self, item_id: String) -> u64 {
        self.begin_panel_request(SecurityPanelRequestKind::PublicKey, item_id)
    }

    pub(in crate::features) fn finish_public_key_request(
        &mut self,
        request_id: u64,
        item_id: &str,
    ) -> bool {
        self.finish_panel_request(request_id, SecurityPanelRequestKind::PublicKey, item_id)
    }

    pub(in crate::features) fn finish_password_request(
        &mut self,
        request_id: u64,
        item_id: &str,
    ) -> bool {
        self.finish_panel_request(request_id, SecurityPanelRequestKind::Password, item_id)
    }

    pub(in crate::features) fn finish_otp_request(
        &mut self,
        request_id: u64,
        item_id: &str,
    ) -> bool {
        self.finish_panel_request(request_id, SecurityPanelRequestKind::Otp, item_id)
    }

    pub(in crate::features) fn finish_credential_request(
        &mut self,
        request_id: u64,
        item_id: &str,
    ) -> bool {
        self.finish_panel_request(request_id, SecurityPanelRequestKind::Credential, item_id)
    }

    pub(in crate::features) fn finish_delete_request(
        &mut self,
        request_id: u64,
        item_id: &str,
    ) -> bool {
        self.finish_panel_request(request_id, SecurityPanelRequestKind::Delete, item_id)
    }

    pub(in crate::features) fn finish_reorder_request(&mut self, request_id: u64) -> bool {
        self.finish_panel_request(request_id, SecurityPanelRequestKind::Reorder, "")
    }

    fn begin_panel_request(&mut self, kind: SecurityPanelRequestKind, item_id: String) -> u64 {
        self.panel.request_id = self.panel.request_id.wrapping_add(1).max(1);
        self.panel.pending_request = Some(SecurityPanelRequest {
            id: self.panel.request_id,
            kind,
            item_id,
        });
        self.panel.request_id
    }

    fn finish_panel_request(
        &mut self,
        request_id: u64,
        kind: SecurityPanelRequestKind,
        item_id: &str,
    ) -> bool {
        let matches = self.panel.pending_request.as_ref().is_some_and(|request| {
            request.id == request_id && request.kind == kind && request.item_id == item_id
        });
        if matches {
            self.panel.pending_request = None;
        }
        matches
    }

    pub(in crate::features) fn set_credential_drop_target(
        &mut self,
        target: Option<SecurityCredentialDropTarget>,
    ) {
        self.panel.credential_drop_target = target;
    }

    pub(in crate::features) fn reordered_credentials(
        &self,
        source_id: &str,
        target_id: &str,
        after: bool,
    ) -> Option<Vec<SavedCredential>> {
        if source_id == target_id {
            return None;
        }
        let source = self
            .catalog
            .credentials
            .iter()
            .find(|entry| entry.id == source_id)?
            .clone();
        let mut next = self
            .catalog
            .credentials
            .iter()
            .filter(|entry| entry.id != source_id)
            .cloned()
            .collect::<Vec<_>>();
        let target = next.iter().position(|entry| entry.id == target_id)?;
        next.insert(if after { target + 1 } else { target }, source);
        for (index, entry) in next.iter_mut().enumerate() {
            entry.sort_order = index as i32;
        }
        Some(next)
    }

    pub(in crate::features) fn clear_revealed_for_deleted(
        &mut self,
        kind: SecurityAuthTab,
        id: &str,
    ) {
        match kind {
            SecurityAuthTab::Otp => {
                self.revealed.otp_codes.remove(id);
                self.panel.visible_otp_ids.remove(id);
            }
            SecurityAuthTab::Passwords => {
                self.revealed.passwords.remove(id);
            }
            SecurityAuthTab::Credentials => {
                self.revealed.credentials.remove(id);
            }
            SecurityAuthTab::Keys | SecurityAuthTab::KnownHosts => {}
        }
    }

    pub(in crate::features) fn key_editor(&self) -> Option<&SecurityKeyEditorState> {
        self.editors.key.as_ref()
    }

    pub(in crate::features) fn key_editor_mut(&mut self) -> Option<&mut SecurityKeyEditorState> {
        self.editors.key.as_mut()
    }

    pub(in crate::features) fn key_editor_focus(&self) -> &FocusHandle {
        &self.editors.key_focus
    }

    pub(in crate::features) fn otp_editor(&self) -> Option<&SecurityOtpEditorState> {
        self.editors.otp.as_ref()
    }

    pub(in crate::features) fn otp_editor_mut(&mut self) -> Option<&mut SecurityOtpEditorState> {
        self.editors.otp.as_mut()
    }

    pub(in crate::features) fn otp_editor_focus(&self) -> &FocusHandle {
        &self.editors.otp_focus
    }

    pub(in crate::features) fn otp_qr_importing(&self) -> bool {
        self.editors.otp_qr_importing
    }

    pub(in crate::features) fn password_editor(&self) -> Option<&SecurityPasswordEditorState> {
        self.editors.password.as_ref()
    }

    pub(in crate::features) fn password_editor_mut(
        &mut self,
    ) -> Option<&mut SecurityPasswordEditorState> {
        self.editors.password.as_mut()
    }

    pub(in crate::features) fn password_editor_focus(&self) -> &FocusHandle {
        &self.editors.password_focus
    }

    pub(in crate::features) fn credential_editor(&self) -> Option<&SecurityCredentialEditorState> {
        self.editors.credential.as_ref()
    }

    pub(in crate::features) fn credential_editor_mut(
        &mut self,
    ) -> Option<&mut SecurityCredentialEditorState> {
        self.editors.credential.as_mut()
    }

    pub(in crate::features) fn credential_editor_focus(&self) -> &FocusHandle {
        &self.editors.credential_focus
    }

    pub(in crate::features) fn open_key_editor(
        &mut self,
        editor: SecurityKeyEditorState,
        status: String,
    ) {
        self.clear_editors();
        self.editors.key = Some(editor);
        self.status = status;
    }

    pub(in crate::features) fn open_otp_editor(
        &mut self,
        editor: SecurityOtpEditorState,
        status: String,
    ) {
        self.clear_editors();
        self.editors.otp = Some(editor);
        self.status = status;
    }

    pub(in crate::features) fn open_password_editor(
        &mut self,
        editor: SecurityPasswordEditorState,
        status: String,
    ) {
        self.clear_editors();
        self.editors.password = Some(editor);
        self.status = status;
    }

    pub(in crate::features) fn open_credential_editor(
        &mut self,
        editor: SecurityCredentialEditorState,
        status: String,
    ) {
        self.clear_editors();
        self.editors.credential = Some(editor);
        self.status = status;
    }

    pub(in crate::features) fn finish_key_editor(&mut self, status: String) {
        self.editors.key = None;
        self.status = status;
    }

    pub(in crate::features) fn finish_otp_editor(&mut self, status: String) {
        self.editors.otp = None;
        self.status = status;
    }

    pub(in crate::features) fn finish_password_editor(&mut self, status: String) {
        self.editors.password = None;
        self.status = status;
    }

    pub(in crate::features) fn finish_credential_editor(&mut self, status: String) {
        self.editors.credential = None;
        self.status = status;
    }

    pub(in crate::features) fn begin_otp_qr_import(&mut self, status: String) -> Option<u64> {
        if self.editors.otp_qr_importing || self.editors.otp.is_some() {
            return None;
        }
        self.editors.otp_qr_request_id = self.editors.otp_qr_request_id.wrapping_add(1).max(1);
        self.editors.otp_qr_importing = true;
        self.status = status;
        Some(self.editors.otp_qr_request_id)
    }

    pub(in crate::features) fn finish_otp_qr_import(&mut self, request_id: u64) -> bool {
        if !self.editors.otp_qr_importing || self.editors.otp_qr_request_id != request_id {
            return false;
        }
        self.editors.otp_qr_importing = false;
        true
    }

    pub(in crate::features) fn apply_editor_input(&mut self, id: &str, text: String) -> bool {
        match id {
            "key-name" | "key-data" | "key-path" | "key-cert-data" | "key-cert-path"
            | "key-passphrase" => {
                let Some(editor) = self.key_editor_mut() else {
                    return false;
                };
                match id {
                    "key-name" => editor.name = text,
                    "key-data" => {
                        editor.key_data = text.into();
                        if !editor.key_data.trim().is_empty() {
                            editor.key_file_path.clear();
                        }
                    }
                    "key-path" => {
                        editor.key_file_path = text;
                        if !editor.key_file_path.trim().is_empty() {
                            editor.key_data.expose_secret_mut().clear();
                        }
                    }
                    "key-cert-data" => {
                        editor.cert_data = text.into();
                        if !editor.cert_data.trim().is_empty() {
                            editor.cert_file_path.clear();
                        }
                    }
                    "key-cert-path" => {
                        editor.cert_file_path = text;
                        if !editor.cert_file_path.trim().is_empty() {
                            editor.cert_data.expose_secret_mut().clear();
                        }
                    }
                    _ => editor.passphrase = text.into(),
                }
            }
            "pw-name" | "pw-username" | "pw-value" => {
                let Some(editor) = self.password_editor_mut() else {
                    return false;
                };
                match id {
                    "pw-name" => editor.name = text,
                    "pw-username" => editor.username = text,
                    _ => editor.password = text.into(),
                }
            }
            "otp-issuer" | "otp-username" | "otp-secret" | "otp-digits" | "otp-period"
            | "otp-counter" => {
                let Some(editor) = self.otp_editor_mut() else {
                    return false;
                };
                match id {
                    "otp-issuer" => editor.issuer = text,
                    "otp-username" => editor.username = text,
                    "otp-secret" => editor.secret = text.into(),
                    "otp-digits" => editor.digits = digits_only(&text),
                    "otp-period" => editor.period = digits_only(&text),
                    _ => editor.counter = digits_only(&text),
                }
            }
            "cred-name" | "cred-user" | "cred-pass" | "cred-user-re" | "cred-pass-re" => {
                let Some(editor) = self.credential_editor_mut() else {
                    return false;
                };
                match id {
                    "cred-name" => editor.name = text,
                    "cred-user" => editor.username = text,
                    "cred-pass" => editor.password = text.into(),
                    "cred-user-re" => editor.username_prompt_regex = text,
                    _ => editor.password_prompt_regex = text,
                }
            }
            _ => return false,
        }
        true
    }

    fn clear_editors(&mut self) {
        self.editors.key = None;
        self.editors.otp = None;
        self.editors.password = None;
        self.editors.credential = None;
        self.editors.busy = false;
        self.editors.otp_qr_importing = false;
        self.editors.otp_qr_request_id = self.editors.otp_qr_request_id.wrapping_add(1).max(1);
    }
}

fn digits_only(text: &str) -> String {
    text.chars().filter(char::is_ascii_digit).collect()
}

/// Panel transitions that only rearrange security UI state.
///
/// These live on the state rather than on `NyaTermApp` so closing an editor or
/// locking secrets cannot reach any other app state. Callers own the redraw.
impl SecurityFeatureState {
    pub(in crate::features) fn close_key_editor(&mut self) {
        self.editors.key = None;
        self.status = "SSH key editor closed".to_string();
    }

    pub(in crate::features) fn close_otp_editor(&mut self) {
        self.editors.otp = None;
        self.status = "OTP editor closed".to_string();
    }

    pub(in crate::features) fn close_password_editor(&mut self) {
        self.editors.password = None;
        self.status = "password editor closed".to_string();
    }

    pub(in crate::features) fn close_credential_editor(&mut self) {
        self.editors.credential = None;
        self.status = "credential editor closed".to_string();
    }

    pub(in crate::features) fn cycle_otp_algorithm(&mut self) {
        if let Some(editor) = self.editors.otp.as_mut() {
            editor.algorithm = match editor.algorithm.as_str() {
                "SHA1" => "SHA256".to_string(),
                "SHA256" => "SHA512".to_string(),
                _ => "SHA1".to_string(),
            };
        }
    }

    pub(in crate::features) fn close_unlock_prompt(&mut self) {
        self.unlock.prompt_open = false;
        self.unlock.draft.expose_secret_mut().clear();
        self.unlock.error = None;
        self.unlock.busy = false;
    }

    /// Drops every revealed secret and every editor holding one.
    ///
    pub(in crate::features) fn lock_secrets(&mut self) {
        self.unlock.secrets_unlocked = false;
        self.revealed.passwords.clear();
        self.revealed.credentials.clear();
        self.revealed.private_key = None;
        self.revealed.otp_codes.clear();
        self.panel.visible_otp_ids.clear();
        self.panel.otp_refresh_armed = false;
        self.panel.credential_drop_target = None;
        self.panel.pending_request = None;
        self.editors.key = None;
        self.editors.otp = None;
        self.editors.password = None;
        self.editors.credential = None;
        self.editors.busy = false;
        self.editors.otp_qr_importing = false;
        self.editors.otp_qr_request_id = self.editors.otp_qr_request_id.wrapping_add(1).max(1);
        self.unlock.pending_action = None;
        self.close_unlock_prompt();
        self.unlock.master_required_prompt_open = false;
        self.status = "secrets locked".to_string();
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use gpui::TestAppContext;
    use nyaterm_core::{OtpEntry, SavedCredential, SavedPassword, SshKey};

    use super::{SecurityCatalogState, SecurityFeatureFocus, SecurityFeatureState};
    use crate::models::{
        SecurityKeyEditorState, SecurityPasswordEditorState, SecurityUnlockAction,
    };

    fn security_state() -> SecurityFeatureState {
        let cx = TestAppContext::single();
        let focus = || cx.update(|cx| cx.focus_handle());
        SecurityFeatureState::new(
            SecurityCatalogState::new(Vec::new(), Vec::new(), Vec::new(), Vec::new()),
            true,
            "ready".to_string(),
            SecurityFeatureFocus {
                key_editor: focus(),
                otp_editor: focus(),
                password_editor: focus(),
                credential_editor: focus(),
                unlock: focus(),
                screen_lock: focus(),
            },
        )
    }

    #[test]
    fn catalog_replacement_and_clear_update_all_security_collections() {
        let mut security = security_state();

        security.replace_catalog(
            vec![SshKey {
                id: "key-id".to_string(),
                name: "key".to_string(),
                key: None,
                cert: None,
                passphrase: None,
                key_file_path: None,
                cert_file_path: None,
                has_key_data: false,
                has_cert_data: false,
            }],
            vec![OtpEntry {
                id: "otp-id".to_string(),
                otp_type: "totp".to_string(),
                issuer: String::new(),
                username: String::new(),
                secret: None,
                algorithm: "SHA1".to_string(),
                digits: 6,
                period: 30,
                counter: 0,
                has_secret: false,
            }],
            vec![SavedPassword {
                username: String::new(),
                id: "password-id".to_string(),
                name: "password".to_string(),
                password: None,
                has_password: false,
            }],
            vec![SavedCredential {
                id: "credential-id".to_string(),
                sort_order: 0,
                name: "credential".to_string(),
                username: String::new(),
                password: None,
                username_prompt_regex: None,
                password_prompt_regex: None,
                enabled: true,
                has_password: false,
            }],
        );

        assert_eq!(security.ssh_keys().len(), 1);
        assert_eq!(security.otp_entries().len(), 1);
        assert_eq!(security.passwords().len(), 1);
        assert_eq!(security.credentials().len(), 1);

        security.catalog =
            SecurityCatalogState::new(Vec::new(), Vec::new(), Vec::new(), Vec::new());

        assert!(security.ssh_keys().is_empty());
        assert!(security.otp_entries().is_empty());
        assert!(security.passwords().is_empty());
        assert!(security.credentials().is_empty());
    }

    #[test]
    fn known_hosts_reject_stale_reads_and_serialize_mutations_across_tabs() {
        let mut state = security_state();
        let first = state.begin_known_hosts_load();
        let second = state.begin_known_hosts_load();
        assert!(!state.apply_known_hosts_load(first, Vec::new()));
        assert!(!state.fail_known_hosts_load(first, "stale".into()));
        assert!(state.known_hosts_loading());
        assert!(state.begin_known_hosts_clear().is_none());
        assert!(state.apply_known_hosts_load(second, Vec::new()));
        let delete = state
            .begin_known_host_delete("host".into())
            .expect("delete");
        assert!(state.known_hosts_busy());
        assert!(state.begin_known_hosts_clear().is_none());
        assert!(state.begin_known_host_delete("other".into()).is_none());
        assert!(!state.apply_known_hosts_load(second, Vec::new()));
        state.set_auth_tab(crate::models::SecurityAuthTab::Keys);
        assert!(!state.finish_known_host_delete(delete, "other"));
        assert!(state.finish_known_host_delete(delete, "host"));
        let clear = state.begin_known_hosts_clear().expect("clear");
        assert!(!state.finish_known_hosts_clear(delete));
        assert!(state.finish_known_hosts_clear(clear));
        assert!(!state.known_hosts_busy());
    }

    #[test]
    fn public_key_clipboard_completion_is_invalidated_by_lock_or_tab_change() {
        let mut state = security_state();
        let first = state.begin_public_key_request("key".into());
        state.lock_secrets();
        assert!(!state.finish_public_key_request(first, "key"));
        let second = state.begin_public_key_request("key".into());
        state.set_auth_tab(crate::models::SecurityAuthTab::KnownHosts);
        assert!(!state.finish_public_key_request(second, "key"));
    }

    #[test]
    fn opening_an_editor_replaces_the_previous_editor() {
        let mut security = security_state();

        security.open_password_editor(
            SecurityPasswordEditorState {
                id: None,
                name: String::new(),
                username: String::new(),
                password: nyaterm_core::SecretString::default(),
                has_password: false,
                show_password: false,
                error: None,
            },
            "password editor opened".to_string(),
        );
        assert!(security.password_editor().is_some());

        security.open_key_editor(
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
                show_passphrase: false,
                has_key_data: false,
                has_cert_data: false,
                error: None,
            },
            "SSH key editor opened".to_string(),
        );

        assert!(security.password_editor().is_none());
        assert!(security.key_editor().is_some());
        assert!(security.otp_editor().is_none());
        assert!(security.credential_editor().is_none());
        assert!(security.apply_editor_input("key-name", "new key".to_string()));
        assert_eq!(
            security.key_editor().map(|editor| editor.name.as_str()),
            Some("new key")
        );

        security.finish_key_editor("saved".to_string());
        assert!(security.key_editor().is_none());
        assert_eq!(security.status, "saved");
    }

    #[test]
    fn otp_qr_import_admission_is_owned_by_security_state() {
        let mut security = security_state();

        let request_id = security
            .begin_otp_qr_import("scanning".to_string())
            .expect("first import should start");
        assert!(
            security
                .begin_otp_qr_import("duplicate".to_string())
                .is_none()
        );
        assert!(security.otp_qr_importing());
        assert_eq!(security.status, "scanning");

        assert!(security.finish_otp_qr_import(request_id));
        assert!(!security.otp_qr_importing());
    }

    #[test]
    fn qr_and_private_key_results_are_rejected_after_replacement() {
        let mut security = security_state();

        let qr_request = security
            .begin_otp_qr_import("scanning".to_string())
            .expect("QR import should start");
        security.set_auth_tab(crate::models::SecurityAuthTab::Otp);
        assert!(!security.finish_otp_qr_import(qr_request));

        let first = security.begin_private_key_view("first".to_string());
        security.close_private_key_view();
        let second = security.begin_private_key_view("second".to_string());
        assert_ne!(first, second);
        assert!(!security.finish_private_key_view(first, Ok("stale".to_string())));
        assert!(security.finish_private_key_view(second, Ok("current".to_string())));
    }

    #[test]
    fn otp_visibility_and_refresh_are_cleared_when_the_tab_changes() {
        let mut security = security_state();
        security.replace_catalog(
            Vec::new(),
            vec![OtpEntry {
                id: "totp-id".to_string(),
                otp_type: "totp".to_string(),
                issuer: String::new(),
                username: String::new(),
                secret: None,
                algorithm: "SHA1".to_string(),
                digits: 6,
                period: 30,
                counter: 0,
                has_secret: true,
            }],
            Vec::new(),
            Vec::new(),
        );

        assert!(security.toggle_otp_code_visible("totp-id".to_string()));
        assert_eq!(security.visible_totp_ids(), vec!["totp-id".to_string()]);
        assert!(security.arm_otp_refresh());
        assert!(!security.arm_otp_refresh());

        security.set_auth_tab(crate::models::SecurityAuthTab::Passwords);
        assert!(security.visible_totp_ids().is_empty());
        assert!(security.arm_otp_refresh());
    }

    #[test]
    fn credential_reorder_reindexes_entries_without_mutating_catalog() {
        let mut security = security_state();
        let credential = |id: &str, sort_order| SavedCredential {
            id: id.to_string(),
            sort_order,
            name: id.to_string(),
            username: String::new(),
            password: None,
            username_prompt_regex: None,
            password_prompt_regex: None,
            enabled: true,
            has_password: false,
        };
        security.replace_catalog(
            Vec::new(),
            Vec::new(),
            Vec::new(),
            vec![credential("a", 0), credential("b", 1), credential("c", 2)],
        );

        let reordered = security
            .reordered_credentials("a", "b", true)
            .expect("valid reorder");
        assert_eq!(
            reordered
                .iter()
                .map(|entry| (entry.id.as_str(), entry.sort_order))
                .collect::<Vec<_>>(),
            vec![("b", 0), ("a", 1), ("c", 2)]
        );
        assert_eq!(security.credentials()[0].id, "a");
    }

    #[test]
    fn closing_or_switching_tabs_invalidates_editor_requests_and_clears_drafts() {
        let mut security = security_state();
        security.open_password_editor(
            SecurityPasswordEditorState {
                id: None,
                name: "draft".to_string(),
                username: String::new(),
                password: "secret".to_string().into(),
                has_password: false,
                show_password: true,
                error: None,
            },
            "editing".to_string(),
        );
        let request_id = security
            .begin_editor_request()
            .expect("editor request should start");

        security.set_auth_tab(crate::models::SecurityAuthTab::Otp);
        assert!(security.password_editor().is_none());
        assert!(!security.finish_editor_request(request_id));
    }

    #[test]
    fn closing_unlock_prompt_invalidates_pending_verification() {
        let mut security = security_state();
        security.show_unlock_prompt();
        security.apply_unlock_input("secret".to_string());
        let (request_id, _) = security
            .begin_unlock_request()
            .expect("unlock request should start");

        security.cancel_unlock_prompt();
        assert!(!security.finish_unlock_request(request_id));
        assert!(security.unlock_draft().is_empty());
    }

    #[test]
    fn unlock_transitions_keep_pending_action_until_success_or_explicit_close() {
        let mut security = security_state();
        security.set_pending_unlock_action(Some(SecurityUnlockAction::RevealPassword(
            "password-id".to_string(),
        )));
        security.show_unlock_prompt();
        security.apply_unlock_input("attempt".to_string());

        security.reject_unlock("wrong password".to_string(), "unlock rejected");
        assert!(security.unlock_draft().is_empty());
        assert!(security.unlock_error().is_some());
        assert!(security.unlock_prompt_open());

        assert_eq!(
            security.complete_unlock(),
            Some(SecurityUnlockAction::RevealPassword(
                "password-id".to_string()
            ))
        );
        assert!(security.secrets_unlocked());
        assert!(!security.unlock_prompt_open());
        assert!(security.unlock_error().is_none());

        security.set_pending_unlock_action(Some(SecurityUnlockAction::RevealCredential(
            "credential-id".to_string(),
        )));
        security.show_master_required_prompt();
        assert!(security.master_required_prompt_open());
        assert_eq!(security.complete_unlock(), None);
    }

    #[test]
    fn locking_secrets_clears_all_revealed_secrets_and_otp_codes() {
        let mut security = security_state();
        security.reveal_password("password-id".to_string(), "value".to_string());
        security.reveal_credential("credential-id".to_string(), "value".to_string());
        security.reveal_otp_code("otp-id".to_string(), "123456".to_string());

        security.lock_secrets();

        assert!(!security.secrets_unlocked());
        assert!(security.revealed_password("password-id").is_none());
        assert!(security.revealed_credential("credential-id").is_none());
        assert!(security.revealed_otp_code("otp-id").is_none());
    }

    #[test]
    fn successful_save_hides_only_the_returned_secret_id() {
        let mut security = security_state();
        security.reveal_password("saved".to_string(), "old value".to_string());
        security.reveal_password("other".to_string(), "keep".to_string());
        security.reveal_credential("saved".to_string(), "old credential".to_string());
        security.reveal_credential("other".to_string(), "keep credential".to_string());

        assert!(security.hide_revealed_password("saved"));
        assert!(security.hide_revealed_credential("saved"));
        assert!(security.revealed_password("saved").is_none());
        assert!(security.revealed_credential("saved").is_none());
        assert_eq!(security.revealed_password("other"), Some("keep"));
        assert_eq!(
            security.revealed_credential("other"),
            Some("keep credential")
        );
    }

    #[test]
    fn private_key_results_are_ignored_after_close_or_replacement() {
        let mut security = security_state();
        let first = security.begin_private_key_view("first".to_string());
        security.close_private_key_view();
        assert!(!security.finish_private_key_view(first, Ok("secret".to_string())));

        let second = security.begin_private_key_view("second".to_string());
        assert!(security.finish_private_key_view(second, Ok("new secret".to_string())));
        assert_eq!(
            security.private_key_view(),
            Some(("second", "new secret", None))
        );
    }

    #[test]
    fn screen_lock_lifecycle_clears_password_and_resets_activity() {
        let mut security = security_state();
        let cx = TestAppContext::single();
        let previous_focus = cx.update(|cx| cx.focus_handle());
        security.remember_screen_lock_focus(Some(previous_focus.clone()));
        security.set_screen_lock_password_draft("secret".to_string(), "ready".to_string());
        security.screen_lock.last_user_activity_at =
            std::time::Instant::now() - Duration::from_secs(60);
        let stale_activity = security.screen_lock.last_user_activity_at;

        security.activate_screen_lock("locked".to_string());
        security.remember_screen_lock_focus(None);
        assert!(security.take_screen_lock_restore_focus().is_some());
        assert!(security.screen_locked());
        assert!(security.screen_lock_password_draft().is_empty());
        assert_eq!(security.screen_lock_status(), "locked");

        security.set_screen_lock_password_draft("retry".to_string(), "retry".to_string());
        security.deactivate_screen_lock();
        assert!(!security.screen_locked());
        assert!(security.screen_lock_password_draft().is_empty());
        assert!(security.screen_lock_status().is_empty());
        assert!(security.screen_lock.last_user_activity_at > stale_activity);
    }

    #[test]
    fn locked_screen_does_not_record_background_activity() {
        let mut security = security_state();
        security.activate_screen_lock("locked".to_string());
        let locked_at = security.screen_lock.last_user_activity_at;

        security.record_screen_lock_user_activity();

        assert_eq!(security.screen_lock.last_user_activity_at, locked_at);
    }
}
