//! settings security state and transitions.

use super::{MasterPasswordView, SettingsFeatureState};

impl SettingsFeatureState {
    pub(in crate::features) fn master_password(&self) -> MasterPasswordView<'_> {
        MasterPasswordView {
            enabled: self.master_password.enabled,
            draft: self.master_password.draft.expose_secret(),
        }
    }
    pub(in crate::features) fn restore_master_password_draft(
        &mut self,
        enabled: bool,
        draft: nyaterm_core::SecretString,
    ) {
        self.master_password.enabled = enabled;
        self.master_password.draft = draft;
    }
    pub(in crate::features) fn toggle_master_password(
        &mut self,
        cloud_sync_enabled: bool,
    ) -> Result<bool, &'static str> {
        self.master_password.toggle(cloud_sync_enabled)
    }
    pub(in crate::features) fn edit_master_password_draft(&mut self, text: String) -> bool {
        self.master_password.edit_draft(text)
    }
}
