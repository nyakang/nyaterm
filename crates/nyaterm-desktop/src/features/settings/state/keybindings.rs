//! settings keybindings state and transitions.

use super::SettingsFeatureState;
use gpui::FocusHandle;
use std::collections::HashMap;

pub(super) struct KeybindingSettingsState {
    pub(super) recording_id: Option<crate::shortcuts::ShortcutId>,
    pub(super) pending_binding: Option<crate::shortcuts::ShortcutBinding>,
    pub(super) search_draft: String,
    pub(super) focus: FocusHandle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::features) struct KeybindingPresentationState {
    pub recording_id: Option<String>,
    pub pending_keys: Option<String>,
    pub search_draft: String,
}

impl SettingsFeatureState {
    pub(in crate::features) fn set_keybindings(&mut self, keybindings: HashMap<String, String>) {
        self.summary.keybindings = keybindings;
    }
    pub(in crate::features) fn keybinding_presentation(&self) -> KeybindingPresentationState {
        KeybindingPresentationState {
            recording_id: self
                .keybindings
                .recording_id
                .map(|id| id.as_str().to_string()),
            pending_keys: self
                .keybindings
                .pending_binding
                .as_ref()
                .map(crate::shortcuts::ShortcutBinding::canonical),
            search_draft: self.keybindings.search_draft.clone(),
        }
    }
    pub(in crate::features) fn keybinding_focus(&self) -> &FocusHandle {
        &self.keybindings.focus
    }
    pub(in crate::features) fn begin_keybinding_recording(
        &mut self,
        shortcut_id: crate::shortcuts::ShortcutId,
    ) {
        self.keybindings.recording_id = Some(shortcut_id);
        self.keybindings.pending_binding = None;
    }
    pub(in crate::features) fn cancel_keybinding_recording(&mut self) {
        self.keybindings.recording_id = None;
        self.keybindings.pending_binding = None;
    }
    pub(in crate::features) fn keybinding_recording_id(
        &self,
    ) -> Option<crate::shortcuts::ShortcutId> {
        self.keybindings.recording_id
    }
    pub(in crate::features) fn pending_keybinding(
        &self,
    ) -> Option<&crate::shortcuts::ShortcutBinding> {
        self.keybindings.pending_binding.as_ref()
    }
    pub(in crate::features) fn set_pending_keybinding(
        &mut self,
        binding: Option<crate::shortcuts::ShortcutBinding>,
    ) {
        self.keybindings.pending_binding = binding;
    }
    pub(in crate::features) fn finish_keybinding_recording(&mut self) {
        self.cancel_keybinding_recording();
    }
    pub(in crate::features) fn set_keybinding_search(&mut self, text: String) {
        self.keybindings.search_draft = text;
    }
    pub(in crate::features) fn clear_keybinding_search(&mut self) {
        self.keybindings.search_draft.clear();
    }
}
