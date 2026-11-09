//! Authoritative settings state and focused transition modules.

use self::appearance::AppearanceSettingsState;
use self::keybindings::KeybindingSettingsState;
use self::keyword_highlights::KeywordHighlightSettingsState;
use self::persistence::SettingsPersistenceSlot;
use self::prompts::SettingsPromptState;
use self::search_engines::SearchEngineSettingsState;
use crate::features::FontCatalogState;
use crate::features::settings::catalog::{SettingsMasterPasswordState, StoreStatus};
use crate::models::KeywordHighlightEditorField;
use gpui::FocusHandle;
use nyaterm_core::{AppSettingsSummary, KeywordHighlightConfig};
use std::collections::{HashMap, HashSet};

mod appearance;
mod keybindings;
mod keyword_highlights;
mod persistence;
mod prompts;
mod search_engines;
mod security;
mod values;

pub(in crate::features) use keybindings::KeybindingPresentationState;
pub(in crate::features) use keyword_highlights::KeywordHighlightPresentationState;
pub(in crate::features) use persistence::SettingsPersistenceDomain;
pub(in crate::features) use search_engines::{SearchEngineMenu, SearchEnginePresentationState};

pub(in crate::features) struct SettingsFeatureState {
    /// Compatibility-sensitive values loaded and persisted through `nyaterm-core`.
    summary: AppSettingsSummary,
    keyword_config: KeywordHighlightConfig,
    keyword_persistence_dirty: bool,
    master_password: SettingsMasterPasswordState,
    store_status: StoreStatus,
    search_engines: SearchEngineSettingsState,
    keyword_highlights: KeywordHighlightSettingsState,
    appearance: AppearanceSettingsState,
    keybindings: KeybindingSettingsState,
    prompts: SettingsPromptState,
    persistence: HashMap<SettingsPersistenceDomain, SettingsPersistenceSlot>,
    draft_dirty_domains: HashSet<SettingsPersistenceDomain>,
    panel_refresh_requested: bool,
}

pub(in crate::features) struct SettingsFeatureFocus {
    pub search_engine: FocusHandle,
    pub keyword_highlight: FocusHandle,
    pub keybindings: FocusHandle,
}

pub(in crate::features) struct SettingsFeatureInit {
    pub summary: AppSettingsSummary,
    pub keyword_config: KeywordHighlightConfig,
    pub store_path: String,
    pub store_message: String,
    pub store_ready: bool,
    pub ui_font_options: Vec<String>,
    pub terminal_font_options: Vec<String>,
}

pub(in crate::features) struct StoreStatusView<'a> {
    pub path: &'a str,
    pub message: &'a str,
    pub ready: bool,
}

/// Borrowed staged master-password state. Deliberately does not implement
/// `Debug` so the secret draft cannot be exposed through aggregate logging.
pub(in crate::features) struct MasterPasswordView<'a> {
    pub enabled: bool,
    pub draft: &'a str,
}

pub(in crate::features) struct UiLayoutSettingsUpdate {
    pub left_panel_width: u32,
    pub right_panel_width: u32,
    pub transfer_height: u32,
    pub quick_command_height: u32,
    pub quick_command_visible: bool,
    pub serial_send_height: u32,
    pub serial_send_visible: bool,
    pub active_left_panel: Option<String>,
    pub active_right_panel: Option<String>,
    pub left_panel_collapsed: bool,
    pub right_panel_collapsed: bool,
    pub saved_connections_sort_mode: String,
    pub saved_connections_expanded_group_ids: Vec<String>,
    pub start_workspace_mode: String,
    pub asset_sort_key: Option<String>,
    pub asset_sort_direction: Option<String>,
    pub activity_bar_left_top: Vec<String>,
    pub activity_bar_left_bottom: Vec<String>,
    pub activity_bar_right_top: Vec<String>,
    pub activity_bar_right_bottom: Vec<String>,
    pub activity_bar_show_labels: bool,
    pub activity_bar_hidden_items: Vec<String>,
    pub panel_multi_open: bool,
    pub panel_open_mode: String,
    pub left_open_panels: Vec<String>,
    pub right_open_panels: Vec<String>,
    pub panel_stack_sizes: HashMap<String, u32>,
}

#[cfg(test)]
mod tests;

impl SettingsFeatureState {
    pub(in crate::features) fn new(init: SettingsFeatureInit, focus: SettingsFeatureFocus) -> Self {
        let SettingsFeatureInit {
            summary,
            keyword_config,
            store_path,
            store_message,
            store_ready,
            ui_font_options,
            terminal_font_options,
        } = init;
        crate::i18n::apply_locale(&summary.language);
        let master_password = SettingsMasterPasswordState::new(summary.has_master_password);
        let palette = nyaterm_ui::resolve_theme_palette(&summary.theme, &summary.custom_themes);
        let terminal_palette = Self::resolve_terminal_palette(&summary);
        let theme_options = nyaterm_ui::available_themes(&summary.custom_themes).into();
        let theme_diagnostics = summary.custom_themes.diagnostics().into();
        Self {
            summary,
            keyword_config,
            keyword_persistence_dirty: false,
            master_password,
            store_status: StoreStatus {
                path: store_path,
                message: store_message,
                ready: store_ready,
            },
            search_engines: SearchEngineSettingsState {
                expanded_index: None,
                icon_picker_index: None,
                focus: focus.search_engine,
            },
            keyword_highlights: KeywordHighlightSettingsState {
                expanded_id: None,
                edit_id: None,
                edit_field: KeywordHighlightEditorField::Name,
                focus: focus.keyword_highlight,
            },
            appearance: AppearanceSettingsState {
                palette,
                theme_options,
                theme_diagnostics,
                terminal_palette,
                font_catalog: FontCatalogState::new(ui_font_options, terminal_font_options),
                designer: Default::default(),
            },
            keybindings: KeybindingSettingsState {
                recording_id: None,
                pending_binding: None,
                search_draft: String::new(),
                focus: focus.keybindings,
            },
            prompts: SettingsPromptState::default(),
            persistence: HashMap::new(),
            draft_dirty_domains: HashSet::new(),
            panel_refresh_requested: false,
        }
    }

    pub(in crate::features) fn request_panel_refresh(&mut self) -> bool {
        if self.panel_refresh_requested {
            return false;
        }
        self.panel_refresh_requested = true;
        true
    }

    pub(in crate::features) fn clear_panel_refresh_request(&mut self) {
        self.panel_refresh_requested = false;
    }

    pub(in crate::features) fn rebase_master_password(&mut self) {
        self.master_password.reset(self.summary.has_master_password);
    }

    pub(in crate::features) fn summary(&self) -> &AppSettingsSummary {
        &self.summary
    }

    pub(in crate::features) fn replace_summary(&mut self, summary: AppSettingsSummary) {
        self.summary = summary;
        self.refresh_theme_palettes();
        crate::i18n::apply_locale(&self.summary.language);
    }
}
