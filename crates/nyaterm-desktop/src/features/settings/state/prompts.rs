//! settings prompts state and transitions.

use super::SettingsFeatureState;
use crate::models::{
    ConfigPathPromptKind, DiagnosticsPathPromptKind, KeywordHighlightPathPromptKind,
    SnapshotPasswordPromptState,
};

#[derive(Default)]
pub(super) struct SettingsPromptState {
    pub(super) config_path: Option<ConfigPathPromptKind>,
    pub(super) diagnostics_path: Option<DiagnosticsPathPromptKind>,
    pub(super) keyword_highlight_path: Option<KeywordHighlightPathPromptKind>,
    pub(super) snapshot_password: Option<SnapshotPasswordPromptState>,
    pub(super) quit_session_count: Option<usize>,
}

impl SettingsFeatureState {
    pub(in crate::features) fn begin_quit_confirmation(&mut self, count: usize) -> bool {
        if self.prompts.quit_session_count.is_some() {
            return false;
        }
        self.prompts.quit_session_count = Some(count);
        true
    }
    pub(in crate::features) fn take_quit_confirmation(&mut self) -> Option<usize> {
        self.prompts.quit_session_count.take()
    }
    pub(in crate::features) fn config_path_prompt_active(&self) -> bool {
        self.prompts.config_path.is_some()
    }
    pub(in crate::features) fn begin_config_path_prompt(
        &mut self,
        kind: ConfigPathPromptKind,
    ) -> bool {
        if self.prompts.config_path.is_some() {
            return false;
        }
        self.prompts.config_path = Some(kind);
        true
    }
    pub(in crate::features) fn finish_config_path_prompt(
        &mut self,
        kind: ConfigPathPromptKind,
    ) -> bool {
        if self.prompts.config_path != Some(kind) {
            return false;
        }
        self.prompts.config_path = None;
        true
    }
    pub(in crate::features) fn begin_diagnostics_path_prompt(&mut self) -> bool {
        if self.prompts.diagnostics_path.is_some() {
            return false;
        }
        self.prompts.diagnostics_path = Some(DiagnosticsPathPromptKind::Export);
        true
    }
    pub(in crate::features) fn finish_diagnostics_path_prompt(&mut self) -> bool {
        if self.prompts.diagnostics_path != Some(DiagnosticsPathPromptKind::Export) {
            return false;
        }
        self.prompts.diagnostics_path = None;
        true
    }
    pub(in crate::features) fn begin_keyword_highlight_path_prompt(&mut self) -> bool {
        if self.prompts.keyword_highlight_path.is_some() {
            return false;
        }
        self.prompts.keyword_highlight_path = Some(KeywordHighlightPathPromptKind::Import);
        true
    }
    pub(in crate::features) fn finish_keyword_highlight_path_prompt(&mut self) -> bool {
        if self.prompts.keyword_highlight_path != Some(KeywordHighlightPathPromptKind::Import) {
            return false;
        }
        self.prompts.keyword_highlight_path = None;
        true
    }
    pub(in crate::features) fn snapshot_password_prompt(
        &self,
    ) -> Option<SnapshotPasswordPromptState> {
        self.prompts.snapshot_password.clone()
    }
    pub(in crate::features) fn snapshot_password_prompt_active(&self) -> bool {
        self.prompts.snapshot_password.is_some()
    }
    pub(in crate::features) fn begin_snapshot_password_prompt(
        &mut self,
        kind: crate::models::SnapshotPasswordPromptKind,
    ) -> bool {
        if self.prompts.config_path.is_some() || self.prompts.snapshot_password.is_some() {
            return false;
        }
        self.prompts.snapshot_password = Some(SnapshotPasswordPromptState {
            kind,
            value: nyaterm_core::SecretString::default(),
        });
        true
    }
    pub(in crate::features) fn take_snapshot_password_prompt(
        &mut self,
    ) -> Option<SnapshotPasswordPromptState> {
        self.prompts.snapshot_password.take()
    }
    pub(in crate::features) fn restore_snapshot_password_prompt(
        &mut self,
        kind: crate::models::SnapshotPasswordPromptKind,
    ) {
        self.prompts.snapshot_password = Some(SnapshotPasswordPromptState {
            kind,
            value: nyaterm_core::SecretString::default(),
        });
    }
    pub(in crate::features) fn apply_snapshot_password_input(&mut self, text: String) -> bool {
        let Some(state) = self.prompts.snapshot_password.as_mut() else {
            return false;
        };
        state.value = text.into();
        true
    }
}
