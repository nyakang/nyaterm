//! settings keyword_highlights state and transitions.

use super::SettingsFeatureState;
use crate::models::KeywordHighlightEditorField;
use gpui::FocusHandle;
use nyaterm_core::{KeywordHighlightConfig, KeywordHighlightRule};

pub(super) struct KeywordHighlightSettingsState {
    pub(super) expanded_id: Option<String>,
    pub(super) edit_id: Option<String>,
    pub(super) edit_field: KeywordHighlightEditorField,
    pub(super) focus: FocusHandle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::features) struct KeywordHighlightPresentationState {
    pub expanded_id: Option<String>,
    pub edit_id: Option<String>,
    pub edit_field: KeywordHighlightEditorField,
}

impl SettingsFeatureState {
    pub(in crate::features) fn keyword_config(&self) -> &KeywordHighlightConfig {
        &self.keyword_config
    }
    pub(in crate::features) fn replace_keyword_config(&mut self, config: KeywordHighlightConfig) {
        self.keyword_config = config;
    }
    pub(in crate::features) fn toggle_keyword_highlights(&mut self) -> bool {
        self.keyword_config.enabled = !self.keyword_config.enabled;
        self.keyword_config.enabled
    }
    pub(in crate::features) fn toggle_keyword_highlight_builtin(&mut self, rule_id: String) {
        let enabled = self
            .keyword_config
            .builtin_rules
            .get(&rule_id)
            .copied()
            .unwrap_or(true);
        self.keyword_config.builtin_rules.insert(rule_id, !enabled);
    }
    pub(in crate::features) fn toggle_keyword_highlight_rule(&mut self, rule_id: &str) -> bool {
        let Some(rule) = self
            .keyword_config
            .rules
            .iter_mut()
            .find(|rule| rule.id == rule_id)
        else {
            return false;
        };
        rule.enabled = !rule.enabled;
        true
    }
    pub(in crate::features) fn set_keyword_highlight_rule_color(
        &mut self,
        rule_id: &str,
        dark: bool,
        color: String,
    ) -> bool {
        let Some(rule) = self
            .keyword_config
            .rules
            .iter_mut()
            .find(|rule| rule.id == rule_id)
        else {
            return false;
        };
        if dark {
            rule.color_dark = color;
        } else {
            rule.color_light = color;
        }
        true
    }
    pub(in crate::features) fn apply_keyword_highlight_rule_input(
        &mut self,
        rule_id: &str,
        field: KeywordHighlightEditorField,
        text: String,
    ) -> bool {
        let Some(rule) = self
            .keyword_config
            .rules
            .iter_mut()
            .find(|rule| rule.id == rule_id)
        else {
            return false;
        };
        match field {
            KeywordHighlightEditorField::Name => rule.name = text,
            KeywordHighlightEditorField::Patterns => {
                rule.patterns = text.split('\n').map(ToOwned::to_owned).collect();
            }
            KeywordHighlightEditorField::ColorDark => rule.color_dark = text,
            KeywordHighlightEditorField::ColorLight => rule.color_light = text,
        }
        self.begin_keyword_highlight_edit(rule_id.to_string(), field);
        true
    }
    /// Toggle field-level highlighting for recognised command output.
    pub(in crate::features) fn toggle_line_role_highlights(&mut self) -> Option<bool> {
        if !self.keyword_config.enabled {
            return None;
        }
        self.summary.ui_highlight_line_roles = !self.summary.ui_highlight_line_roles;
        Some(self.summary.ui_highlight_line_roles)
    }
    pub(in crate::features) fn keyword_highlight_presentation(
        &self,
    ) -> KeywordHighlightPresentationState {
        KeywordHighlightPresentationState {
            expanded_id: self.keyword_highlights.expanded_id.clone(),
            edit_id: self.keyword_highlights.edit_id.clone(),
            edit_field: self.keyword_highlights.edit_field,
        }
    }
    pub(in crate::features) fn keyword_highlight_focus(&self) -> &FocusHandle {
        &self.keyword_highlights.focus
    }
    pub(in crate::features) fn clear_keyword_highlight_edit(&mut self) {
        self.keyword_highlights.edit_id = None;
    }
    /// Returns ids whose registry-backed inputs should be discarded.
    pub(in crate::features) fn toggle_keyword_highlight_expanded(
        &mut self,
        rule_id: String,
    ) -> Vec<String> {
        let mut forgotten = Vec::new();
        if self.keyword_highlights.expanded_id.as_deref() == Some(rule_id.as_str()) {
            self.keyword_highlights.expanded_id = None;
            self.keyword_highlights.edit_id = None;
            forgotten.push(rule_id);
        } else {
            if let Some(previous_id) = self.keyword_highlights.expanded_id.replace(rule_id) {
                forgotten.push(previous_id);
            }
            self.keyword_highlights.edit_id = None;
        }
        forgotten
    }
    pub(in crate::features) fn begin_keyword_highlight_edit(
        &mut self,
        rule_id: String,
        field: KeywordHighlightEditorField,
    ) {
        self.keyword_highlights.expanded_id = Some(rule_id.clone());
        self.keyword_highlights.edit_id = Some(rule_id);
        self.keyword_highlights.edit_field = field;
    }
    pub(super) fn remove_keyword_highlight_rule_reference(&mut self, rule_id: &str) {
        if self.keyword_highlights.expanded_id.as_deref() == Some(rule_id) {
            self.keyword_highlights.expanded_id = None;
        }
        if self.keyword_highlights.edit_id.as_deref() == Some(rule_id) {
            self.keyword_highlights.edit_id = None;
        }
    }
    pub(in crate::features) fn remove_keyword_highlight_rule(&mut self, rule_id: &str) -> bool {
        let previous_len = self.keyword_config.rules.len();
        self.keyword_config.rules.retain(|rule| rule.id != rule_id);
        if self.keyword_config.rules.len() == previous_len {
            return false;
        }
        self.remove_keyword_highlight_rule_reference(rule_id);
        true
    }
    pub(in crate::features) fn add_keyword_highlight_rule(&mut self, rule: KeywordHighlightRule) {
        let id = rule.id.clone();
        self.keyword_config.rules.push(rule);
        self.begin_keyword_highlight_edit(id, KeywordHighlightEditorField::Name);
    }
}
