use std::sync::Arc;

use gpui::Context;

use crate::features::{FontCatalogPresentation, NyaTermApp};
use crate::models::SettingsTab;

use super::panel::{
    AiSettingsPresentation, CloudSyncPresentation, MasterPasswordPresentation, SettingsChrome,
    SettingsPresentation, SettingsSectionPresentation, SettingsSnapshot,
    TransferSettingsPresentation, TranslationPresentation,
};

impl NyaTermApp {
    pub(in crate::features) fn request_settings_panel_refresh(&mut self, cx: &mut Context<Self>) {
        if !self.settings.request_panel_refresh() {
            return;
        }
        self.defer_app_update(cx, |app, cx| {
            app.flush_settings_panel_snapshots(cx);
        });
    }

    pub(in crate::features) fn flush_settings_panel_snapshots(&mut self, cx: &mut Context<Self>) {
        self.settings.clear_panel_refresh_request();
        let main = self.build_settings_snapshot();
        self.settings_panel
            .clone()
            .update(cx, |panel, cx| panel.set_snapshot(main, cx));
    }

    pub(in crate::features) fn build_settings_snapshot(&self) -> SettingsSnapshot {
        let active_tab = self.shell.settings_active_tab();
        let backup_prompt = self.settings.snapshot_password_prompt();
        let draft_dirty = self.settings_draft_dirty() || self.settings.theme_designer().dirty();
        SettingsSnapshot {
            chrome: SettingsChrome {
                palette: self.theme_palette(),
            },
            active_tab,
            settings: SettingsPresentation {
                summary: Arc::new(self.settings.summary().clone()),
                theme_options: self.settings.theme_options(),
                theme_diagnostics: self.settings.theme_diagnostics(),
                theme_designer: self.settings.theme_designer().clone(),
                keyword_config: Arc::new(self.settings.keyword_config().clone()),
                search_engine_presentation: self.settings.search_engine_presentation(),
                keyword_highlight_presentation: self.settings.keyword_highlight_presentation(),
                keybinding_presentation: self.settings.keybinding_presentation(),
                master_password: {
                    let master_password = self.settings.master_password();
                    MasterPasswordPresentation {
                        enabled: master_password.enabled,
                        draft: master_password.draft.to_owned().into(),
                    }
                },
                font_catalog: FontCatalogPresentation::new(
                    self.settings.font_catalog_state(),
                    self.settings.font_catalog_generation(),
                    self.settings.font_catalog_snapshot(),
                ),
                search_engine_focus: self.settings.search_engine_focus().clone(),
                keyword_highlight_focus: self.settings.keyword_highlight_focus().clone(),
                keybinding_focus: self.settings.keybinding_focus().clone(),
                snapshot_password_prompt: self.settings.snapshot_password_prompt(),
                snapshot_password_prompt_active: self.settings.snapshot_password_prompt_active(),
                config_path_prompt_active: self.settings.config_path_prompt_active(),
                terminal_theme_is_dark: self.terminal_theme_is_dark(),
                panel_multi_open: self.shell.panel_multi_open(),
            },
            ai: AiSettingsPresentation {
                providers: self.ai.provider_view().clone(),
                config: Arc::new(nyaterm_core::mask_ai_settings(self.ai.pending_settings())),
                header_errors: Arc::new(self.ai.settings_header_errors()),
                manual_model_drafts: Arc::new(self.ai.settings_manual_model_drafts().clone()),
                credential_draft_key_ids: Arc::new(
                    self.ai
                        .settings_credential_secret_drafts()
                        .iter()
                        .filter(|(_, secret)| !secret.trim().is_empty())
                        .map(|(id, _)| id.clone())
                        .collect(),
                ),
                agent_management: self.ai.agent_management_view().clone(),
            },
            cloud_sync: CloudSyncPresentation {
                v4_preview: self.cloud_sync.v4_summary(),
                settings: Arc::new(self.cloud_sync.settings().clone()),
                state: self.cloud_sync.state().clone(),
                history: self.cloud_sync.history().to_vec(),
                pending_settings: self.cloud_sync.pending_settings(),
                secret_draft: self.cloud_sync.secret_draft().clone(),
                status: self.cloud_sync.status().to_string(),
                job_running: self.cloud_sync.job_running(),
                live_state: self.cloud_sync.live_state(),
                conflict: self.cloud_sync.conflict().cloned(),
                github_auth: self.cloud_sync.github_auth().clone(),
            },
            translation: {
                let (settings, secret_draft) = self.translation.settings_draft_snapshot();
                TranslationPresentation {
                    settings,
                    secret_draft,
                }
            },
            transfer: TransferSettingsPresentation {
                duplicate_policy: self.transfer.duplicate_policy(),
            },
            text_inputs: self.text_input_fields_snapshot(),
            number_inputs: self.number_input_fields_snapshot(),
            draft_open: self.shell.has_settings_draft(),
            draft_dirty,
            validation_error: draft_dirty
                .then(|| self.pending_settings_validation_error())
                .flatten(),
            backup_prompt,
            expanded_groups: Arc::from(
                ["workspace", "terminal_session", "ai_group"]
                    .into_iter()
                    .filter(|group| self.shell.settings_group_is_expanded(group))
                    .map(str::to_string)
                    .collect::<Vec<_>>(),
            ),
            section: match active_tab {
                SettingsTab::General => SettingsSectionPresentation::General,
                SettingsTab::Appearance => SettingsSectionPresentation::Appearance,
                SettingsTab::Interaction => SettingsSectionPresentation::Interaction,
                SettingsTab::Keybindings => SettingsSectionPresentation::Keybindings,
                SettingsTab::TerminalGeneral => SettingsSectionPresentation::TerminalGeneral,
                SettingsTab::RemoteMonitoring => SettingsSectionPresentation::RemoteMonitoring,
                SettingsTab::Search => SettingsSectionPresentation::Search,
                SettingsTab::Translation => SettingsSectionPresentation::Translation,
                SettingsTab::AiGeneral => SettingsSectionPresentation::AiGeneral,
                SettingsTab::AiModels => SettingsSectionPresentation::AiModels,
                SettingsTab::AiRules => SettingsSectionPresentation::AiRules,
                SettingsTab::AiAgents => SettingsSectionPresentation::AiAgents,
                SettingsTab::Transfer => SettingsSectionPresentation::Transfer,
                SettingsTab::Security => SettingsSectionPresentation::Security,
                SettingsTab::SyncBackup => SettingsSectionPresentation::SyncBackup,
            },
        }
    }
}
