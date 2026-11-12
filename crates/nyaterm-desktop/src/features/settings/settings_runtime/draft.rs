use crate::features::NyaTermApp;
use crate::features::app_state::SettingsDraftSnapshot;
use crate::models::TranslationSecretDraft;
use gpui::Context;
use nyaterm_core::AppSettingsSummary;
use nyaterm_files::sftp_transfer_types::SftpDuplicatePolicy;
use nyaterm_store::StoreDomain;
use rust_i18n::t;

impl NyaTermApp {
    pub(in crate::features) fn begin_settings_draft(&mut self, cx: &mut Context<Self>) {
        if self.shell.has_settings_draft() {
            return;
        }
        self.settings.clear_draft_dirty_domains();
        let (translation_settings, translation_secret_draft) =
            self.translation.settings_draft_snapshot();
        let (cloud_sync_settings, cloud_sync_secret_draft) =
            self.cloud_sync.settings_draft_snapshot();
        let (ai_settings, ai_model_draft, ai_base_url_draft, ai_secret_draft) =
            self.ai.settings_draft_snapshot();
        let master_password = self.settings.master_password();
        self.shell
            .set_settings_draft_snapshot(SettingsDraftSnapshot {
                revisions: self.process_state.read(cx).settings_draft_revisions(),
                settings: self.settings.summary().clone(),
                ai_settings,
                ai_model_draft,
                ai_base_url_draft,
                ai_secret_draft,
                cloud_sync_settings,
                cloud_sync_secret_draft,
                translation_settings,
                translation_secret_draft,
                keyword_highlights: self.settings.keyword_config().clone(),
                master_password_enabled: master_password.enabled,
                master_password_draft: master_password.draft.to_owned().into(),
            });
        self.request_settings_panel_refresh(cx);
    }

    pub(in crate::features) fn settings_draft_dirty(&self) -> bool {
        let Some(snapshot) = self.shell.settings_draft_snapshot() else {
            return false;
        };
        let master_password = self.settings.master_password();
        !settings_draft_matches(&snapshot.settings, self.settings.summary())
            || !self.ai.settings_draft_matches(
                &snapshot.ai_settings,
                &snapshot.ai_model_draft,
                &snapshot.ai_base_url_draft,
                &snapshot.ai_secret_draft,
            )
            || !self.cloud_sync.settings_draft_matches(
                &snapshot.cloud_sync_settings,
                &snapshot.cloud_sync_secret_draft,
            )
            || !self.translation.settings_draft_matches(
                &snapshot.translation_settings,
                &snapshot.translation_secret_draft,
            )
            || snapshot.keyword_highlights != *self.settings.keyword_config()
            || snapshot.master_password_enabled != master_password.enabled
            || snapshot.master_password_draft.expose_secret() != master_password.draft
    }

    /// Returns true when a settings save should stay in the in-memory draft.
    pub(in crate::features) fn defer_settings_persistence(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        self.request_settings_panel_refresh(cx);
        if !self.shell.has_settings_draft() {
            return false;
        }
        self.settings
            .update_store_status(t!("settings.draftChanged").to_string(), true);
        self.shell
            .set_status(t!("settings.draftChanged").to_string());
        cx.notify();
        true
    }

    pub(in crate::features) fn defer_settings_domain_persistence(
        &mut self,
        domain: crate::features::settings::SettingsPersistenceDomain,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.defer_settings_persistence(cx) {
            return false;
        }
        self.settings.mark_draft_domain_dirty(domain);
        true
    }

    pub(in crate::features) fn pending_settings_cloud_error(&self) -> Option<String> {
        let settings = self.cloud_sync.pending_settings();
        if !settings.enabled {
            return None;
        }
        let master_password = self.settings.master_password();
        if !master_password.enabled {
            return Some(t!("settings.syncEnableMasterPasswordFirst").to_string());
        }
        if !self.settings.summary().has_master_password && master_password.draft.is_empty() {
            return Some(t!("settings.syncMasterPasswordRequired").to_string());
        }
        let missing_key = match settings.provider.as_str() {
            "webdav" if settings.webdav.endpoint.trim().is_empty() => {
                Some("settings.webdavEndpointRequired")
            }
            "s3" if settings.s3.endpoint.trim().is_empty() => Some("settings.s3EndpointRequired"),
            "s3" if settings.s3.bucket.trim().is_empty() => Some("settings.s3BucketRequired"),
            "s3" if settings
                .s3
                .access_key_id
                .as_deref()
                .unwrap_or("")
                .trim()
                .is_empty()
                != settings
                    .s3
                    .secret_access_key
                    .as_deref()
                    .unwrap_or("")
                    .trim()
                    .is_empty() =>
            {
                Some("settings.s3CredentialsIncomplete")
            }
            "gitee_snippet" if settings.gitee_snippet.api_endpoint.trim().is_empty() => {
                Some("settings.giteeSnippetEndpointRequired")
            }
            "gitee_snippet" if settings.gitee_snippet.gist_id.trim().is_empty() => {
                Some("settings.giteeSnippetIdRequired")
            }
            "gitee_snippet"
                if settings
                    .gitee_snippet
                    .access_token
                    .as_deref()
                    .unwrap_or("")
                    .trim()
                    .is_empty() =>
            {
                Some("settings.giteeSnippetTokenRequired")
            }
            "google_drive" => drive_validation_key(
                settings.google_drive.refresh_token.as_deref(),
                settings.google_drive.client_id.as_deref(),
                settings.google_drive.client_secret.as_deref(),
            ),
            "onedrive" => drive_validation_key(
                settings.onedrive.refresh_token.as_deref(),
                settings.onedrive.client_id.as_deref(),
                settings.onedrive.client_secret.as_deref(),
            ),
            "aliyun_drive" => drive_validation_key(
                settings.aliyun_drive.refresh_token.as_deref(),
                settings.aliyun_drive.client_id.as_deref(),
                settings.aliyun_drive.client_secret.as_deref(),
            ),
            "github_gist" if settings.github_gist.gist_id.trim().is_empty() => {
                Some("settings.githubGistRequired")
            }
            "github_gist"
                if settings
                    .github_gist
                    .access_token
                    .as_deref()
                    .unwrap_or("")
                    .trim()
                    .is_empty() =>
            {
                Some("settings.githubGistTokenRequired")
            }
            _ => None,
        };
        missing_key.map(|key| t!(key).to_string())
    }

    /// Reasons the settings draft must not be persisted right now.
    ///
    /// The master-password case is the one the report cares about: enabling the
    /// switch with no stored password and no draft leaves nothing to save, so the
    /// post-apply rebase which reads `has_master_password` from disk would turn the
    /// switch back off. Block instead of silently reverting.
    pub(in crate::features) fn pending_settings_validation_error(&self) -> Option<String> {
        if let Some(error) = self.pending_settings_cloud_error() {
            return Some(error);
        }
        if let Some(error) = self.ai_provider_validation_error() {
            return Some(error);
        }
        let master_password = self.settings.master_password();
        if master_password.enabled
            && !self.settings.summary().has_master_password
            && master_password.draft.is_empty()
        {
            return Some(t!("settings.masterPasswordEnterBeforeEnabling").to_string());
        }
        None
    }

    pub(in crate::features) fn block_cloud_sync_for_settings_draft(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.settings_draft_dirty() {
            return false;
        }
        self.cloud_sync
            .set_status(t!("settings.applySettingsFirst"));
        self.shell.set_status(self.cloud_sync.status().to_string());
        cx.notify();
        true
    }

    pub(in crate::features) fn block_import_for_settings_draft(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.settings_draft_dirty() {
            return false;
        }
        self.shell.set_status(t!("settings.syncApplyOrCancelFirst"));
        self.settings
            .update_store_status(self.shell.status().to_string(), false);
        cx.notify();
        true
    }

    pub(in crate::features) fn rebase_open_settings_draft(&mut self, cx: &mut Context<Self>) {
        if !self.shell.has_settings_draft() {
            return;
        }
        self.shell.clear_settings_draft_snapshot();
        self.settings.rebase_master_password();
        self.begin_settings_draft(cx);
    }

    pub(in crate::features) fn apply_settings_draft(
        &mut self,
        close_after_apply: bool,
        cx: &mut Context<Self>,
    ) {
        // Theme saves commit independently before resuming this intent. Reject an
        // invalid ordinary draft before starting that flow, but allow theme-only saves.
        if self.settings_draft_dirty()
            && let Some(error) = self.pending_settings_validation_error()
        {
            self.settings.update_store_status(error.clone(), false);
            self.shell
                .set_status(t!("settings.applyBlocked", detail = error));
            cx.notify();
            self.request_settings_panel_refresh(cx);
            return;
        }
        if self.settings.theme_designer().dirty() {
            self.theme_designer_action(
                crate::features::settings::theme_designer::ThemeIntent::Apply(close_after_apply),
                cx,
            );
            return;
        }
        if !self.shell.has_settings_draft() {
            if close_after_apply {
                self.finish_settings_page(cx);
            }
            self.request_settings_panel_refresh(cx);
            return;
        }
        self.hide_ai_provider_secret(cx);
        self.ai.invalidate_provider_jobs();
        let settings = self.settings.summary().clone();
        let mut base_settings = self
            .shell
            .settings_draft_snapshot()
            .expect("settings draft checked above")
            .settings
            .clone();
        crate::models::ActivityBarLayoutState::complete_settings_layout(&mut base_settings);
        let base = self
            .shell
            .settings_draft_snapshot()
            .expect("settings draft checked above")
            .clone();
        let settings_domains = self.settings.draft_dirty_domains();
        let settings_changed = !settings_domains.is_empty();
        let keyword_changed = self.settings.keyword_config() != &base.keyword_highlights;
        let ai_changed = !self.ai.settings_draft_matches(
            &base.ai_settings,
            &base.ai_model_draft,
            &base.ai_base_url_draft,
            &base.ai_secret_draft,
        );
        let cloud_changed = !self
            .cloud_sync
            .settings_draft_matches(&base.cloud_sync_settings, &base.cloud_sync_secret_draft);
        let translation_changed = !self
            .translation
            .settings_draft_matches(&base.translation_settings, &base.translation_secret_draft);
        let master_password = self.settings.master_password();
        let master_password_changed = base.master_password_enabled != master_password.enabled
            || base.master_password_draft.expose_secret() != master_password.draft;
        let revisions = self.process_state.read(cx).settings_draft_revisions();
        let settings_revision_conflict =
            (settings_changed || keyword_changed || master_password_changed)
                && revisions.settings != base.revisions.settings;
        let ai_revision_conflict = ai_changed && revisions.ai != base.revisions.ai;
        let cloud_revision_conflict =
            cloud_changed && revisions.cloud_sync != base.revisions.cloud_sync;
        let translation_revision_conflict =
            translation_changed && revisions.translation != base.revisions.translation;
        if settings_revision_conflict
            || ai_revision_conflict
            || cloud_revision_conflict
            || translation_revision_conflict
        {
            let message = t!("settings.changedInAnotherWindow").to_string();
            self.settings.update_store_status(message.clone(), false);
            self.shell.set_status(message);
            self.request_settings_panel_refresh(cx);
            cx.notify();
            return;
        }
        let ai_settings = self.pending_ai_settings();
        let mut cloud_sync_settings = self.cloud_sync.pending_settings();
        let translation_settings = self.translation.pending_settings();
        let keyword_highlights = self.settings.keyword_config().clone();
        let master_password_update = if master_password.draft.is_empty() {
            (self.settings.summary().has_master_password && !master_password.enabled)
                .then_some(None)
        } else {
            Some(Some(master_password.draft.to_string()))
        };
        let password_rotation =
            (crate::features::sync::effective_protocol_version(&base.cloud_sync_settings) == 4
                && base.settings.has_master_password)
                .then(|| {
                    master_password_update
                        .as_ref()
                        .and_then(|value| value.as_ref())
                        .cloned()
                })
                .flatten()
                .map(nyaterm_core::SecretString::from);
        let sync_configuration_changed = cloud_changed || master_password_update.is_some();
        if password_rotation.is_some() && cloud_changed {
            let message = t!("settings.syncV3RotateSeparately").to_string();
            self.settings.update_store_status(message.clone(), false);
            self.shell.set_status(message);
            self.request_settings_panel_refresh(cx);
            cx.notify();
            return;
        }
        self.settings
            .update_store_status(t!("settings.applying").to_string(), false);
        self.submit_store_request_with_password_rotation(
            0,
            password_rotation,
            nyaterm_store::store_mutation(StoreDomain::Settings, move |store| {
                let conflict = || {
                    nyaterm_store::StorageError::InvalidData(
                        t!("settings.changedInAnotherWindow").to_string(),
                    )
                };
                let mut shared_settings = settings.clone();
                if settings_changed {
                    let persisted_settings = store.load_app_settings_summary()?;
                    shared_settings.ui_left_panel_width = persisted_settings.ui_left_panel_width;
                    shared_settings.ui_right_panel_width = persisted_settings.ui_right_panel_width;
                    shared_settings.ui_quick_cmd_height = persisted_settings.ui_quick_cmd_height;
                    shared_settings.ui_active_left_panel =
                        persisted_settings.ui_active_left_panel.clone();
                    shared_settings.ui_active_right_panel =
                        persisted_settings.ui_active_right_panel.clone();
                    shared_settings.ui_left_panel_collapsed =
                        persisted_settings.ui_left_panel_collapsed;
                    shared_settings.ui_right_panel_collapsed =
                        persisted_settings.ui_right_panel_collapsed;
                    let mut persisted = persisted_settings;
                    crate::models::ActivityBarLayoutState::complete_settings_layout(&mut persisted);
                    // These fields are workspace-local projections, not shared settings.
                    persisted.ui_left_panel_width = base_settings.ui_left_panel_width;
                    persisted.ui_right_panel_width = base_settings.ui_right_panel_width;
                    persisted.ui_quick_cmd_height = base_settings.ui_quick_cmd_height;
                    persisted.ui_active_left_panel = base_settings.ui_active_left_panel.clone();
                    persisted.ui_active_right_panel = base_settings.ui_active_right_panel.clone();
                    persisted.ui_left_panel_collapsed = base_settings.ui_left_panel_collapsed;
                    persisted.ui_right_panel_collapsed = base_settings.ui_right_panel_collapsed;
                    if persisted != base_settings {
                        return Err(conflict());
                    }
                }
                if keyword_changed && store.load_keyword_highlights()? != base.keyword_highlights {
                    return Err(conflict());
                }
                if ai_changed && store.load_ai_settings()? != base.ai_settings {
                    return Err(conflict());
                }
                let current_cloud = store.load_cloud_sync_settings()?;
                let mut cloud_base = base.cloud_sync_settings.clone();
                nyaterm_core::cloud_sync::credentials::rebase_rotated_tokens(
                    &mut cloud_base,
                    &mut cloud_sync_settings,
                    &current_cloud,
                );
                if cloud_changed && current_cloud != cloud_base {
                    return Err(conflict());
                }
                if translation_changed
                    && store.load_translation_settings()? != base.translation_settings
                {
                    return Err(conflict());
                }
                if let Some(next_password) = master_password_update.as_ref() {
                    store.save_master_password(next_password.as_deref())?;
                }
                for domain in settings_domains {
                    match domain {
                        crate::features::settings::SettingsPersistenceDomain::Diagnostics => {
                            store.save_diagnostics_settings(&shared_settings)?;
                        }
                        crate::features::settings::SettingsPersistenceDomain::General => {
                            store.save_general_settings(&shared_settings)?;
                        }
                        crate::features::settings::SettingsPersistenceDomain::Interaction => {
                            store.save_interaction_settings(&shared_settings)?;
                        }
                        crate::features::settings::SettingsPersistenceDomain::ScreenLock => {
                            store.save_screen_lock_settings(&shared_settings)?;
                        }
                        crate::features::settings::SettingsPersistenceDomain::HostKey => {
                            store.save_host_key_policy(&shared_settings.host_key_policy)?;
                        }
                        crate::features::settings::SettingsPersistenceDomain::Recording => {
                            store.save_recording_settings(&shared_settings)?;
                        }
                        crate::features::settings::SettingsPersistenceDomain::Transfer => {
                            store.save_transfer_settings(&shared_settings)?;
                        }
                        crate::features::settings::SettingsPersistenceDomain::Terminal => {
                            store.save_terminal_settings(&shared_settings)?;
                        }
                        crate::features::settings::SettingsPersistenceDomain::QuickCommands => {
                            store.save_quick_command_ui_settings(&shared_settings)?;
                        }
                        crate::features::settings::SettingsPersistenceDomain::Appearance => {
                            store.save_appearance_settings(&shared_settings)?;
                        }
                        crate::features::settings::SettingsPersistenceDomain::UiLayout => {
                            store.save_ui_layout_settings(&shared_settings)?;
                        }
                        crate::features::settings::SettingsPersistenceDomain::Keybindings => {
                            store.save_keybindings(&shared_settings.keybindings)?;
                        }
                        crate::features::settings::SettingsPersistenceDomain::FileExplorer => {
                            store.save_file_explorer_favorite_dirs(&shared_settings)?;
                        }
                    }
                }
                if settings_changed && !settings.startup_restore_window_layout {
                    store.save_terminal_window_layout(None)?;
                    store.save_workspace_pane_layout(None)?;
                }
                let saved_keyword_highlights = if keyword_changed {
                    store.save_keyword_highlights(&keyword_highlights)?
                } else {
                    store.load_keyword_highlights()?
                };
                let saved_translation_settings = if translation_changed {
                    store.save_translation_settings(translation_settings)?
                } else {
                    store.load_translation_settings()?
                };
                let saved_cloud_sync_settings = if cloud_changed {
                    store.save_cloud_sync_settings(cloud_sync_settings)?
                } else {
                    store.load_cloud_sync_settings()?
                };
                let saved_ai_settings = if ai_changed {
                    store.save_ai_settings(ai_settings)?
                } else {
                    store.load_ai_settings()?
                };
                Ok((
                    store.load_app_settings_summary()?,
                    saved_keyword_highlights,
                    saved_translation_settings,
                    saved_cloud_sync_settings,
                    store.load_cloud_sync_state()?,
                    saved_ai_settings,
                ))
            }),
            move |this, event, cx| match event.outcome {
                Ok((
                    saved_settings,
                    saved_keyword_highlights,
                    saved_translation_settings,
                    saved_cloud_sync_settings,
                    saved_cloud_sync_state,
                    saved_ai_settings,
                )) => {
                    this.apply_gpui_settings(saved_settings.clone(), cx);
                    this.publish_shared_settings(saved_settings, cx);
                    if sync_configuration_changed
                        && let Some(controller) = this.desktop_controller.clone()
                    {
                        cx.defer(move |cx| {
                            let _ = controller.update(cx, |controller, _| {
                                controller.cloud_sync_settings_changed();
                            });
                        });
                    }
                    this.request_shared_state_refresh(crate::app_shell::SharedStateDomain::All, cx);
                    this.settings.rebase_master_password();
                    this.ai.replace_settings_config(saved_ai_settings, true);
                    this.cloud_sync
                        .replace_loaded(saved_cloud_sync_settings, saved_cloud_sync_state);
                    this.translation.replace_settings(
                        saved_translation_settings,
                        TranslationSecretDraft::default(),
                    );
                    this.settings
                        .replace_keyword_config(saved_keyword_highlights);
                    this.sync_ai_drafts_from_active_profile();
                    this.reset_ai_settings_inputs(cx);
                    this.forget_text_inputs("ai.credential.");
                    this.forget_text_inputs("ai.settings.action.");
                    this.forget_text_inputs("ai.settings.manual-model.");
                    this.recording.set_memory_limit(
                        this.settings.summary().recording_memory_limit_bytes as usize,
                    );
                    this.transfer
                        .set_duplicate_policy(SftpDuplicatePolicy::from_legacy_value(
                            &this.settings.summary().transfer_duplicate_strategy,
                        ));
                    this.sync_terminal_encodings_from_settings();
                    this.enforce_terminal_scrollback_limit();
                    if !this
                        .settings
                        .summary()
                        .interaction_command_suggestions_enabled
                    {
                        this.terminal.clear_command_tracking();
                    }
                    if !this
                        .settings
                        .summary()
                        .interaction_credential_autofill_enabled
                    {
                        this.cancel_credential_autofill(cx);
                    }
                    this.invalidate_terminal_cell_metrics(cx);
                    this.refresh_visible_terminal_surfaces(cx);
                    this.shell.clear_settings_draft_snapshot();
                    this.settings.clear_draft_dirty_domains();
                    this.settings
                        .update_store_status(t!("settings.applied").to_string(), true);
                    this.shell.set_status(t!("settings.applied").to_string());
                    if close_after_apply {
                        this.finish_settings_page(cx);
                    } else {
                        this.begin_settings_draft(cx);
                        this.ensure_settings_tab_inputs(this.shell.settings_active_tab(), cx);
                        cx.notify();
                    }
                    this.request_settings_panel_refresh(cx);
                }
                Err(error) => {
                    let message = t!("settings.applyFailed", error = error).to_string();
                    this.settings.update_store_status(message.clone(), false);
                    this.shell.set_status(message);
                    this.request_settings_panel_refresh(cx);
                    cx.notify();
                }
            },
            cx,
        );
    }

    pub(in crate::features) fn cancel_settings(&mut self, cx: &mut Context<Self>) {
        if self.settings.theme_designer().submitting {
            return;
        }
        if self.settings.theme_designer().dirty() {
            self.theme_designer_action(
                crate::features::settings::theme_designer::ThemeIntent::CloseSettings,
                cx,
            );
            return;
        }
        self.settings.theme_designer_mut().reset();
        self.hide_ai_provider_secret(cx);
        if let Some(mut snapshot) = self.shell.take_settings_draft_snapshot() {
            copy_settings_shell_state(&mut snapshot.settings, self.settings.summary());
            self.apply_gpui_settings(snapshot.settings, cx);
            self.ai.restore_settings_draft(
                snapshot.ai_settings,
                snapshot.ai_model_draft,
                snapshot.ai_base_url_draft,
                snapshot.ai_secret_draft,
            );
            self.cloud_sync.replace_settings(
                snapshot.cloud_sync_settings,
                snapshot.cloud_sync_secret_draft,
            );
            self.translation.replace_settings(
                snapshot.translation_settings,
                snapshot.translation_secret_draft,
            );
            self.settings
                .replace_keyword_config(snapshot.keyword_highlights);
            self.settings.restore_master_password_draft(
                snapshot.master_password_enabled,
                snapshot.master_password_draft,
            );
            self.recording
                .set_memory_limit(self.settings.summary().recording_memory_limit_bytes as usize);
            self.transfer
                .set_duplicate_policy(SftpDuplicatePolicy::from_legacy_value(
                    &self.settings.summary().transfer_duplicate_strategy,
                ));
            self.sync_terminal_encodings_from_settings();
            self.invalidate_terminal_cell_metrics(cx);
            self.invalidate_paint_theme_caches();
            self.sync_ai_drafts_from_active_profile();
            self.reset_ai_settings_inputs(cx);
            self.refresh_visible_terminal_surfaces(cx);
        }
        self.settings.clear_draft_dirty_domains();
        self.finish_settings_page(cx);
        self.request_settings_panel_refresh(cx);
    }

    pub(in crate::features) fn confirm_settings_draft(&mut self, cx: &mut Context<Self>) {
        if self.settings_draft_dirty() || self.settings.theme_designer().dirty() {
            self.apply_settings_draft(true, cx);
        } else {
            self.shell.clear_settings_draft_snapshot();
            self.settings.clear_draft_dirty_domains();
            self.finish_settings_page(cx);
        }
        self.request_settings_panel_refresh(cx);
    }

    pub(in crate::features) fn toggle_settings_master_password(&mut self, cx: &mut Context<Self>) {
        self.shell.set_status(
            match self
                .settings
                .toggle_master_password(self.cloud_sync.settings().enabled)
            {
                Ok(true) => t!("settings.masterPasswordEnabled").to_string(),
                Ok(false) => t!("settings.masterPasswordRemovalStaged").to_string(),
                Err(error) => error.to_string(),
            },
        );
        self.request_settings_panel_refresh(cx);
        cx.notify();
    }

    /// Apply an edit from the master password box.
    ///
    /// Like the cloud sync fields, the box must publish into the panel snapshot:
    /// the switch state, the "is set" badge, and the apply button all read the
    /// flushed snapshot, so an edit that only notifies leaves the button grey and
    /// looks like the keystroke never arrived.
    pub(in crate::features) fn apply_settings_master_password(
        &mut self,
        text: String,
        cx: &mut Context<Self>,
    ) {
        if !self.settings.edit_master_password_draft(text) {
            return;
        }
        self.shell
            .set_status(t!("settings.masterPasswordEdited").to_string());
        self.request_settings_panel_refresh(cx);
        cx.notify();
    }

    fn finish_settings_page(&mut self, cx: &mut Context<Self>) {
        let browser_was_visible = self.transfer_browser_is_visible();
        self.collapse_open_search_engine(cx);
        self.hide_secret_inputs(cx);
        self.settings.theme_designer_mut().reset();
        self.forget_text_inputs("theme.designer.");
        self.cancel_github_gist_auth(cx);
        self.hide_ai_provider_secret(cx);
        self.ai.close_settings_editors();
        self.forget_text_inputs("ai.credential.");
        self.settings.clear_keyword_highlight_edit();
        self.forget_text_inputs("ai.settings.action.");
        self.forget_text_inputs("ai.settings.manual-model.");
        self.forget_text_inputs("keyword.highlight.");
        self.close_settings_workspace_tab(cx);
        self.transfer_browser_visibility_changed(browser_was_visible, cx);
        self.shell
            .set_status(t!("settings.settingsClosed").to_string());
        cx.notify();
    }
}

/// Compare a draft against the live settings while ignoring shell state.
///
/// `AppSettingsSummary` doubles as the durable home of layout and session state:
/// `persist_ui_layout` writes panel visibility/collapse/stack state, dock sizes,
/// the active panels, list sorting and expansion, the activity-bar arrangement
/// and the launch surface into it whenever a panel is opened or closed, a divider
/// is dragged, a group is expanded or a list is sorted. A settings draft clones
/// the whole struct, so a direct comparison reports those navigation writes as
/// unsaved setting edits even though the settings page was never touched.
///
/// The fields neutralised below are restored at launch but cannot be edited from
/// the settings page, so they have no place in the draft's dirty state. Every
/// value a settings tab can edit, including the header-status and file-manager
/// preferences that share the same struct, stays in the comparison.
fn settings_draft_matches(snapshot: &AppSettingsSummary, live: &AppSettingsSummary) -> bool {
    shell_state_neutral_settings(snapshot.clone(), live) == *live
}

fn shell_state_neutral_settings(
    mut settings: AppSettingsSummary,
    live: &AppSettingsSummary,
) -> AppSettingsSummary {
    // Normalize just the draft to the current layout; rebuilding defaults and
    // cloning both complete settings would add allocations to every dirty check.
    copy_settings_shell_state(&mut settings, live);
    settings
}

/// The same projection serves comparison and rollback, so a navigation field
/// ignored by dirty tracking cannot be accidentally restored from an old draft.
fn copy_settings_shell_state(settings: &mut AppSettingsSummary, live: &AppSettingsSummary) {
    settings
        .ui_quick_cmd_view_mode
        .clone_from(&live.ui_quick_cmd_view_mode);
    settings
        .ui_quick_cmd_sort_mode
        .clone_from(&live.ui_quick_cmd_sort_mode);
    settings
        .ui_quick_cmd_selected_category
        .clone_from(&live.ui_quick_cmd_selected_category);
    settings
        .ui_saved_connections_sort_mode
        .clone_from(&live.ui_saved_connections_sort_mode);
    settings
        .ui_saved_connections_expanded_group_ids
        .clone_from(&live.ui_saved_connections_expanded_group_ids);
    settings
        .ui_start_workspace_mode
        .clone_from(&live.ui_start_workspace_mode);
    settings
        .ui_asset_sort_key
        .clone_from(&live.ui_asset_sort_key);
    settings
        .ui_asset_sort_direction
        .clone_from(&live.ui_asset_sort_direction);
    settings
        .ui_file_explorer_show_hidden_files
        .clone_from(&live.ui_file_explorer_show_hidden_files);
    settings
        .ui_file_explorer_view_mode
        .clone_from(&live.ui_file_explorer_view_mode);
    settings
        .ui_file_explorer_auto_sync_cwd_connection_ids
        .clone_from(&live.ui_file_explorer_auto_sync_cwd_connection_ids);
    settings
        .ui_file_explorer_favorite_dirs_by_connection_id
        .clone_from(&live.ui_file_explorer_favorite_dirs_by_connection_id);
    settings
        .ui_file_manager
        .open
        .clone_from(&live.ui_file_manager.open);
    settings
        .ui_file_manager
        .left
        .clone_from(&live.ui_file_manager.left);
    settings
        .ui_file_manager
        .right
        .clone_from(&live.ui_file_manager.right);
    settings
        .ui_file_manager
        .split_percent
        .clone_from(&live.ui_file_manager.split_percent);
    settings
        .ui_file_manager
        .active_pane
        .clone_from(&live.ui_file_manager.active_pane);
    settings
        .ui_file_manager
        .queue_expanded
        .clone_from(&live.ui_file_manager.queue_expanded);
    settings
        .ui_file_manager
        .queue_height
        .clone_from(&live.ui_file_manager.queue_height);
    settings
        .ui_file_manager
        .left_view
        .clone_from(&live.ui_file_manager.left_view);
    settings
        .ui_file_manager
        .right_view
        .clone_from(&live.ui_file_manager.right_view);
    settings
        .ui_left_panel_width
        .clone_from(&live.ui_left_panel_width);
    settings
        .ui_right_panel_width
        .clone_from(&live.ui_right_panel_width);
    settings
        .ui_transfer_height
        .clone_from(&live.ui_transfer_height);
    settings
        .ui_quick_cmd_height
        .clone_from(&live.ui_quick_cmd_height);
    settings
        .ui_quick_cmd_visible
        .clone_from(&live.ui_quick_cmd_visible);
    settings
        .ui_serial_send_height
        .clone_from(&live.ui_serial_send_height);
    settings
        .ui_serial_send_clear_after_send
        .clone_from(&live.ui_serial_send_clear_after_send);
    settings
        .ui_serial_send_visible
        .clone_from(&live.ui_serial_send_visible);
    settings
        .ui_active_left_panel
        .clone_from(&live.ui_active_left_panel);
    settings
        .ui_active_right_panel
        .clone_from(&live.ui_active_right_panel);
    settings
        .ui_left_panel_collapsed
        .clone_from(&live.ui_left_panel_collapsed);
    settings
        .ui_right_panel_collapsed
        .clone_from(&live.ui_right_panel_collapsed);
    settings
        .ui_activity_bar_left_top
        .clone_from(&live.ui_activity_bar_left_top);
    settings
        .ui_activity_bar_left_bottom
        .clone_from(&live.ui_activity_bar_left_bottom);
    settings
        .ui_activity_bar_right_top
        .clone_from(&live.ui_activity_bar_right_top);
    settings
        .ui_activity_bar_right_bottom
        .clone_from(&live.ui_activity_bar_right_bottom);
    settings
        .ui_activity_bar_show_labels
        .clone_from(&live.ui_activity_bar_show_labels);
    settings
        .ui_activity_bar_hidden_items
        .clone_from(&live.ui_activity_bar_hidden_items);
    settings
        .ui_panel_multi_open
        .clone_from(&live.ui_panel_multi_open);
    settings
        .ui_panel_open_mode
        .clone_from(&live.ui_panel_open_mode);
    settings
        .ui_left_open_panels
        .clone_from(&live.ui_left_open_panels);
    settings
        .ui_right_open_panels
        .clone_from(&live.ui_right_open_panels);
    settings
        .ui_panel_stack_sizes
        .clone_from(&live.ui_panel_stack_sizes);
}

fn drive_validation_key(
    refresh_token: Option<&str>,
    client_id: Option<&str>,
    client_secret: Option<&str>,
) -> Option<&'static str> {
    if refresh_token.unwrap_or("").trim().is_empty() {
        return Some("settings.driveRefreshTokenRequired");
    }
    if client_id.unwrap_or("").trim().is_empty() {
        return Some("settings.driveClientIdRequired");
    }
    if client_secret.unwrap_or("").trim().is_empty() {
        return Some("settings.driveClientSecretRequired");
    }
    None
}

#[cfg(test)]
mod tests {
    use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
    use crate::features::NyaTermApp;
    use crate::features::settings::SettingsSaveKind;
    use crate::models::HeaderStatusMode;
    use gpui::{AppContext as _, TestAppContext};
    use nyaterm_core::{AppRuntime, RuntimeMode, uuid};

    /// A real store runtime, because these tests are about what reaches disk.
    fn app(cx: &mut TestAppContext) -> gpui::Entity<NyaTermApp> {
        // A uuid rather than a clock reading: these tests run in parallel and
        // Windows' ~15ms clock granularity lets a nanosecond timestamp repeat,
        // which would share one config dir and so one settings database.
        let root = std::env::temp_dir().join(format!(
            "nyaterm-settings-draft-{}-{}",
            std::process::id(),
            uuid()
        ));
        let runtime = AppRuntime::from_parts_for_test(
            RuntimeMode::Portable,
            root.clone(),
            root.join("config"),
            root.join("logs"),
            root.join("cache"),
            None,
        );
        let stores = UiStoreHandles {
            startup_restore: cx.new(|_| StartupRestoreStore::default()),
            overlays: cx.new(|_| OverlayStore::default()),
        };
        cx.new(|cx| NyaTermApp::new(runtime, stores, cx))
    }

    #[test]
    fn command_output_highlighting_draft_cancel_and_apply_preserve_committed_preferences() {
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx);
        let load = |cx: &mut TestAppContext| {
            app.update(cx, |app, _| {
                app.store_blocking_client()
                    .request_fn(nyaterm_store::StoreDomain::Settings, |store| {
                        store.load_app_settings_summary()
                    })
                    .unwrap()
                    .ui_highlight_line_roles
            })
        };
        app.update(&mut cx, |app, cx| {
            let mut keywords = app.settings.keyword_config().clone();
            keywords.enabled = true;
            let keywords = app
                .store_blocking_client()
                .request_fn(nyaterm_store::StoreDomain::Settings, move |store| {
                    store.save_keyword_highlights(&keywords)
                })
                .unwrap();
            app.settings.replace_keyword_config(keywords);
            assert!(app.settings.summary().ui_highlight_line_roles);
            app.begin_settings_draft(cx);
            app.toggle_line_role_highlights(cx);
            assert!(!app.settings.summary().ui_highlight_line_roles);
            assert!(app.settings_draft_dirty());
            app.cancel_settings(cx);
            assert!(app.settings.summary().ui_highlight_line_roles);
        });
        assert!(load(&mut cx));
        for enabled in [false, true, false] {
            app.update(&mut cx, |app, cx| {
                app.begin_settings_draft(cx);
                app.toggle_line_role_highlights(cx);
                assert_eq!(app.settings.summary().ui_highlight_line_roles, enabled);
                assert!(app.settings_draft_dirty());
            });
            assert_eq!(load(&mut cx), !enabled);
            app.update(&mut cx, |app, cx| app.apply_settings_draft(false, cx));
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                cx.run_until_parked();
                if app.read_with(&cx, |app, _| !app.settings_draft_dirty()) {
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "command-output highlighting save did not finish"
                );
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            assert_eq!(load(&mut cx), enabled);
            app.update(&mut cx, |app, _| {
                assert_eq!(app.settings.summary().ui_highlight_line_roles, enabled);
                assert!(!app.settings_draft_dirty());
            });
        }
    }

    #[test]
    fn terminal_wrapping_draft_cancel_and_save_preserve_committed_preferences() {
        use nyaterm_core::models::settings::TerminalWrapMode;
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx);
        app.update(&mut cx, |app, cx| {
            app.begin_settings_draft(cx);
            app.set_terminal_wrap_mode(TerminalWrapMode::None, cx);
            app.toggle_terminal_wrap_markers(cx);
            assert!(app.settings_draft_dirty());
            app.cancel_settings(cx);
            assert_eq!(
                app.settings.summary().terminal_wrap_mode,
                TerminalWrapMode::Character
            );
            assert!(!app.settings.summary().terminal_show_wrap_markers);
            app.begin_settings_draft(cx);
            app.set_terminal_wrap_mode(TerminalWrapMode::None, cx);
            app.toggle_terminal_wrap_markers(cx);
        });
        let before_save = app.update(&mut cx, |app, _| {
            app.store_blocking_client()
                .request_fn(nyaterm_store::StoreDomain::Settings, |store| {
                    store.load_app_settings_summary()
                })
                .unwrap()
        });
        assert_eq!(before_save.terminal_wrap_mode, TerminalWrapMode::Character);
        assert!(!before_save.terminal_show_wrap_markers);
        app.update(&mut cx, |app, cx| app.apply_settings_draft(false, cx));
        cx.run_until_parked();
        let saved = app.update(&mut cx, |app, _| {
            app.store_blocking_client()
                .request_fn(nyaterm_store::StoreDomain::Settings, |store| {
                    store.load_app_settings_summary()
                })
                .unwrap()
        });
        assert_eq!(saved.terminal_wrap_mode, TerminalWrapMode::None);
        assert!(saved.terminal_show_wrap_markers);
    }

    #[test]
    fn provider_draft_cancel_preserves_receipt_and_save_refreshes_it_before_another_sync() {
        use nyaterm_core::{CloudSyncSettings, CloudSyncState};
        use nyaterm_store::StoreDomain;
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx);
        app.update(&mut cx, |app, cx| {
            let (settings, state) = app
                .store_blocking_client()
                .request_fn(StoreDomain::CloudSync, |store| {
                    let settings = store.save_cloud_sync_settings(CloudSyncSettings {
                        provider: "webdav".into(),
                        protocol_version: 4,
                        ..Default::default()
                    })?;
                    let state = CloudSyncState {
                        device_id: "stable".into(),
                        last_synced_payload_hash: Some("previous-provider".into()),
                        last_synced_content_hash: Some("previous-content".into()),
                        ..Default::default()
                    };
                    store.save_cloud_sync_state(&state)?;
                    Ok((settings, state))
                })
                .unwrap();
            app.cloud_sync.replace_loaded(settings, state.clone());
            app.begin_settings_draft(cx);
            app.cloud_sync.select_provider("github_gist");
            assert_eq!(app.cloud_sync.state(), &state);
            app.cancel_settings(cx);
            assert_eq!(app.cloud_sync.settings().provider, "webdav");
            assert_eq!(app.cloud_sync.state(), &state);
        });
        for (provider, protocol) in [("github_gist", 3), ("webdav", 4)] {
            app.update(&mut cx, |app, cx| {
                app.begin_settings_draft(cx);
                app.cloud_sync.select_provider(provider);
                app.apply_settings_draft(false, cx);
            });
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                cx.run_until_parked();
                if app.read_with(&cx, |app, _| {
                    app.cloud_sync.state().last_synced_payload_hash.is_none()
                        && !app.settings_draft_dirty()
                }) {
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "provider save did not finish"
                );
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            app.update(&mut cx, |app, _| {
                assert_eq!(app.cloud_sync.settings().provider, provider);
                assert_eq!(app.cloud_sync.settings().protocol_version, 4);
                assert_eq!(
                    crate::features::sync::effective_protocol_version(app.cloud_sync.settings()),
                    protocol
                );
                assert_eq!(app.cloud_sync.state().device_id, "stable");
                assert!(app.cloud_sync.state().last_synced_payload_hash.is_none());
                assert!(app.cloud_sync.state().last_synced_content_hash.is_none());
                let state = app
                    .store_blocking_client()
                    .request_fn(StoreDomain::CloudSync, |store| {
                        store.load_cloud_sync_state()
                    })
                    .unwrap();
                assert_eq!(&state, app.cloud_sync.state());
            });
        }
    }

    #[test]
    fn row_stripes_draft_cancel_and_apply_preserve_committed_preferences() {
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx);
        app.update(&mut cx, |app, cx| {
            app.begin_settings_draft(cx);
            app.toggle_terminal_zebra_stripes_by_row(cx);
            app.toggle_terminal_zebra_stripes(cx);
            assert!(app.settings.summary().terminal_zebra_stripes_by_row);
            assert!(!app.settings.summary().terminal_zebra_stripes_enabled);
            assert!(app.settings_draft_dirty());
            app.cancel_settings(cx);
            assert!(!app.settings.summary().terminal_zebra_stripes_by_row);
            assert!(app.settings.summary().terminal_zebra_stripes_enabled);
            app.begin_settings_draft(cx);
            app.toggle_terminal_zebra_stripes_by_row(cx);
        });
        let load = |cx: &mut TestAppContext| {
            app.update(cx, |app, _| {
                app.store_blocking_client()
                    .request_fn(nyaterm_store::StoreDomain::Settings, |store| {
                        store.load_app_settings_summary()
                    })
                    .unwrap()
            })
        };
        assert!(!load(&mut cx).terminal_zebra_stripes_by_row);
        app.update(&mut cx, |app, cx| app.apply_settings_draft(false, cx));
        cx.run_until_parked();
        assert!(load(&mut cx).terminal_zebra_stripes_by_row);
    }

    #[test]
    fn scrollback_number_input_retains_business_bounds_and_draft_rollback() {
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx);
        let (input, original) = app.update(&mut cx, |app, cx| {
            let original = app.settings.summary().terminal_scrollback_lines;
            app.begin_settings_draft(cx);
            app.ensure_settings_tab_inputs(crate::models::SettingsTab::TerminalGeneral, cx);
            (
                app.number_input_fields_snapshot()["settings.number.terminal-scrollback-lines"]
                    .clone(),
                original,
            )
        });
        for (text, expected) in [("100000", 100_000), ("999999", 100_000), ("1", 100)] {
            input.update(&mut cx, |_, cx| {
                cx.emit(nyaterm_ui::NyaNumberInputEvent::Changed(text.into()));
            });
            cx.run_until_parked();
            assert_eq!(
                app.read_with(&cx, |app, _| app
                    .settings
                    .summary()
                    .terminal_scrollback_lines),
                expected,
            );
        }
        app.update(&mut cx, |app, cx| {
            app.cancel_settings(cx);
            assert_eq!(app.settings.summary().terminal_scrollback_lines, original);
            assert_eq!(input.read(cx).value(cx), original.to_string());
        });
    }

    #[test]
    fn internal_editor_font_input_and_draft_cancel_restore_the_same_authoritative_value() {
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx);
        let input = app.update(&mut cx, |app, cx| {
            app.begin_settings_draft(cx);
            app.ensure_settings_tab_inputs(crate::models::SettingsTab::Transfer, cx);
            app.number_input_fields_snapshot()["settings.number.transfer-internal-editor-font-size"]
                .clone()
        });
        app.update(&mut cx, |app, cx| {
            let original = app.settings.summary().transfer_internal_editor_font_size;
            app.set_transfer_internal_editor_font_size(original, cx);
            assert!(app.settings.draft_dirty_domains().is_empty());
            app.adjust_transfer_internal_editor_font_size(100, cx);
            assert_eq!(input.read(cx).value(cx), "72");
            app.reset_transfer_internal_editor_font_size(cx);
            assert_eq!(input.read(cx).value(cx), "13");
            app.set_transfer_internal_editor_font_size(21, cx);
            assert!(app.settings_draft_dirty());
            app.cancel_settings(cx);
            assert_eq!(
                app.settings.summary().transfer_internal_editor_font_size,
                original
            );
            assert_eq!(input.read(cx).value(cx), original.to_string());
        });
        let saved = app.update(&mut cx, |app, _| {
            app.store_blocking_client()
                .request_fn(nyaterm_store::StoreDomain::Settings, |store| {
                    store.load_app_settings_summary()
                })
                .unwrap()
        });
        assert_eq!(saved.transfer_internal_editor_font_size, 13);
    }

    #[test]
    fn internal_editor_font_number_input_routes_valid_edits_and_ignores_invalid_text() {
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx);
        let input = app.update(&mut cx, |app, cx| {
            app.begin_settings_draft(cx);
            app.ensure_settings_tab_inputs(crate::models::SettingsTab::Transfer, cx);
            app.number_input_fields_snapshot()["settings.number.transfer-internal-editor-font-size"]
                .clone()
        });
        for (text, expected) in [("20", 20), ("invalid", 20), ("0", 8), ("65535", 72)] {
            input.update(&mut cx, |_, cx| {
                cx.emit(nyaterm_ui::NyaNumberInputEvent::Changed(text.into()))
            });
            cx.run_until_parked();
            assert_eq!(
                app.read_with(&cx, |app, _| app
                    .settings
                    .summary()
                    .transfer_internal_editor_font_size),
                expected
            );
        }
    }

    #[test]
    fn internal_editor_font_saves_through_transfer_settings_with_and_without_a_draft() {
        for draft_open in [false, true] {
            let mut cx = TestAppContext::single();
            let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
            let app = app(&mut cx);
            app.update(&mut cx, |app, cx| {
                if draft_open {
                    app.begin_settings_draft(cx);
                }
                app.set_transfer_internal_editor_font_size(19, cx);
            });
            cx.run_until_parked();
            if draft_open {
                let saved = app.update(&mut cx, |app, _| {
                    app.store_blocking_client()
                        .request_fn(nyaterm_store::StoreDomain::Settings, |store| {
                            store.load_app_settings_summary()
                        })
                        .unwrap()
                });
                assert_eq!(saved.transfer_internal_editor_font_size, 13);
                app.update(&mut cx, |app, cx| app.apply_settings_draft(false, cx));
                cx.run_until_parked();
            }
            let saved = app.update(&mut cx, |app, _| {
                app.store_blocking_client()
                    .request_fn(nyaterm_store::StoreDomain::Settings, |store| {
                        store.load_app_settings_summary()
                    })
                    .unwrap()
            });
            assert_eq!(saved.transfer_internal_editor_font_size, 19);
        }
    }

    fn stored_header_status(
        app: &gpui::Entity<NyaTermApp>,
        cx: &mut TestAppContext,
    ) -> (String, bool) {
        let summary = cx
            .update_entity(app, |app, _| {
                app.store_blocking_client()
                    .request_fn(nyaterm_store::StoreDomain::Settings, |store| {
                        store.load_app_settings_summary()
                    })
            })
            .expect("load stored settings");
        (
            summary.ui_header_status_mode,
            summary.ui_header_status_visible,
        )
    }

    #[test]
    fn editing_or_clearing_external_editor_refreshes_the_draft_and_persists() {
        for next in ["", r"D:\Softwares\Microsoft VS Code\Code.exe"] {
            let mut cx = TestAppContext::single();
            let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
            let app = app(&mut cx);
            let input = cx.update_entity(&app, |app, cx| {
                let saved = app
                    .store_blocking_client()
                    .request_fn(nyaterm_store::StoreDomain::Settings, |store| {
                        let mut settings = store.load_app_settings_summary()?;
                        settings.transfer_default_editor = r"D:\Softwares\Zed\Zed.exe".into();
                        store.save_transfer_settings(&settings)?;
                        store.load_app_settings_summary()
                    })
                    .unwrap();
                app.apply_gpui_settings(saved, cx);
                app.begin_settings_draft(cx);
                app.ensure_settings_tab_inputs(crate::models::SettingsTab::Transfer, cx);
                app.existing_text_input("settings.transfer.default-editor")
                    .unwrap()
            });
            cx.update_entity(&input, |_, cx| {
                cx.emit(nyaterm_ui::NyaInputEvent::Changed(next.to_string()));
            });
            cx.run_until_parked();
            cx.update_entity(&app, |app, cx| {
                assert_eq!(app.settings.summary().transfer_default_editor, next);
                assert!(app.settings_panel.read(cx).snapshot().unwrap().draft_dirty);
                assert!(
                    app.settings
                        .draft_dirty_domains()
                        .contains(&crate::features::settings::SettingsPersistenceDomain::Transfer)
                );
                app.apply_settings_draft(false, cx);
            });
            cx.run_until_parked();
            let saved = cx.update_entity(&app, |app, _| {
                app.store_blocking_client()
                    .request_fn(nyaterm_store::StoreDomain::Settings, |store| {
                        store.load_app_settings_summary()
                    })
                    .unwrap()
            });
            assert_eq!(saved.transfer_default_editor, next);
        }
    }

    #[test]
    fn tray_preference_persists_without_saving_unrelated_settings_drafts() {
        for draft_open in [false, true] {
            let mut cx = TestAppContext::single();
            let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
            let app = app(&mut cx);
            let baseline = cx
                .update_entity(&app, |app, _| {
                    app.store_blocking_client()
                        .request_fn(nyaterm_store::StoreDomain::Settings, |store| {
                            store.load_app_settings_summary()
                        })
                })
                .unwrap();
            cx.update_entity(&app, |app, cx| {
                if draft_open {
                    app.begin_settings_draft(cx);
                    app.toggle_confirm_on_close(cx);
                }
                app.toggle_minimize_to_tray_from_tray(cx);
            });
            cx.run_until_parked();
            let persisted = cx
                .update_entity(&app, |app, _| {
                    app.store_blocking_client()
                        .request_fn(nyaterm_store::StoreDomain::Settings, |store| {
                            store.load_app_settings_summary()
                        })
                })
                .unwrap();
            assert_eq!(persisted.minimize_to_tray, !baseline.minimize_to_tray);
            assert_eq!(persisted.confirm_on_close, baseline.confirm_on_close);
        }
    }

    /// Applying a draft must write the header-status mode it changed.
    ///
    /// `persist_header_status_settings` deliberately defers while a draft is open,
    /// telling the user to apply settings to persist. Apply then reloads the summary
    /// from the store and replaces the in-memory one wholesale, so a value its save
    /// batch failed to write comes back as whatever is still on disk -- which is the
    /// reported symptom: the header reverts the instant settings are applied.
    ///
    /// **The assertion is on disk, not on the summary.** In this harness the store
    /// reply is not delivered inside `run_until_parked`, so the in-memory summary keeps
    /// the value the draft set and an assertion on it passes whether or not anything
    /// was written -- which it did, before the disk check was added. Disk is also the
    /// more fundamental property: a value that never lands there is lost at the next
    /// launch regardless of what this session shows.
    ///
    /// `DateTime` rather than `Session`, because `session` is the stored default -- a
    /// test that set the mode to the default could not tell a successful apply from a
    /// dropped one.
    #[test]
    fn applying_a_draft_persists_the_header_status_mode() {
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx);
        assert_eq!(
            stored_header_status(&app, &mut cx),
            ("session".to_string(), true),
            "fixture baseline"
        );

        cx.update_entity(&app, |app, cx| {
            app.begin_settings_draft(cx);
            app.set_header_status_mode(HeaderStatusMode::DateTime, cx);
            assert_eq!(
                app.settings.summary().ui_header_status_mode,
                "datetime",
                "the draft changes the in-memory summary immediately"
            );
            app.apply_settings_draft(false, cx);
        });
        cx.run_until_parked();

        assert_eq!(
            stored_header_status(&app, &mut cx),
            ("datetime".to_string(), true),
            "apply must write the header status, not leave the stored value behind for \
             its own reload to restore"
        );
    }

    /// Editing only the word separators must still be written when the draft is applied.
    ///
    /// The box used to update memory and nothing else, so no persistence domain was
    /// marked dirty and applying the draft saved no interaction settings at all. The
    /// assertion is on disk for the same reason as the header-status test above.
    #[test]
    fn applying_a_draft_persists_word_separators() {
        const SEPARATORS: &str = " ()[]{}\"':=,;|&<>";
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx);
        let stored = |app: &gpui::Entity<NyaTermApp>, cx: &mut TestAppContext| {
            cx.update_entity(app, |app, _| {
                app.store_blocking_client()
                    .request_fn(nyaterm_store::StoreDomain::Settings, |store| {
                        store.load_app_settings_summary()
                    })
            })
            .expect("load stored settings")
            .interaction_word_separators
        };
        assert_ne!(stored(&app, &mut cx), SEPARATORS, "fixture baseline");

        cx.update_entity(&app, |app, cx| {
            app.begin_settings_draft(cx);
            app.apply_interaction_word_separators(SEPARATORS.to_string(), cx);
            app.apply_settings_draft(false, cx);
        });
        cx.run_until_parked();

        assert_eq!(stored(&app, &mut cx), SEPARATORS);
    }

    #[test]
    fn canceling_a_draft_restores_ui_typography_source() {
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx);

        cx.update_entity(&app, |app, cx| {
            app.sync_component_theme(cx);
            let original_family = app.gpui_ui_font().family;
            let original_size = app.settings.summary().ui_font_size;

            app.begin_settings_draft(cx);
            app.update_ui_font_family("Noto Sans", cx);
            app.set_ui_font_size_from_input(20, cx);

            assert_eq!(app.gpui_ui_font().family, "Noto Sans");
            assert_eq!(app.settings.summary().ui_font_size, 20);

            app.cancel_settings(cx);

            assert_eq!(app.gpui_ui_font().family, original_family);
            assert_eq!(app.settings.summary().ui_font_size, original_size);
        });
    }

    #[test]
    fn native_window_material_preview_cancels_and_apply_persists_independent_opacity() {
        use nyaterm_core::models::window_material::WindowMaterial;
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx);
        cx.update_entity(&app, |app, cx| {
            app.begin_settings_draft(cx);
            app.set_window_material(WindowMaterial::Blurred, cx);
            app.set_window_background_opacity(35, cx);
            assert_eq!(
                app.settings.summary().window_material,
                WindowMaterial::Blurred
            );
            assert_eq!(app.settings.summary().window_background_opacity, 35);
            assert!(app.settings_draft_dirty());
            let stored = app
                .store_blocking_client()
                .request_fn(nyaterm_store::StoreDomain::Settings, |store| {
                    store.load_app_settings_summary()
                })
                .unwrap();
            assert_eq!(stored.window_material, WindowMaterial::Opaque);
            app.cancel_settings(cx);
            assert_eq!(
                app.settings.summary().window_material,
                WindowMaterial::Opaque
            );
            assert_eq!(app.settings.summary().window_background_opacity, 20);
            app.begin_settings_draft(cx);
            app.set_window_material(WindowMaterial::Transparent, cx);
            app.set_window_background_opacity(0, cx);
            app.apply_settings_draft(false, cx);
        });
        cx.run_until_parked();
        cx.update_entity(&app, |app, cx| {
            let stored = app
                .store_blocking_client()
                .request_fn(nyaterm_store::StoreDomain::Settings, |store| {
                    store.load_app_settings_summary()
                })
                .unwrap();
            assert_eq!(stored.window_material, WindowMaterial::Transparent);
            assert_eq!(stored.window_background_opacity, 0);
            assert_eq!(stored.background_content_opacity, 82);
            app.cancel_settings(cx);
        });
    }

    #[test]
    fn window_decorations_preview_cancels_and_apply_persists() {
        use nyaterm_core::models::window_decoration::WindowDecoration;

        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx);
        app.update(&mut cx, |app, cx| {
            app.begin_settings_draft(cx);
            app.set_window_decorations(WindowDecoration::Client, cx);
            assert!(app.settings.draft_dirty_domains().is_empty());
            app.set_window_decorations(WindowDecoration::Server, cx);
            assert_eq!(
                app.window_decoration_preview(),
                Some(WindowDecoration::Server)
            );
            assert!(app.settings_draft_dirty());
            let stored = app
                .store_blocking_client()
                .request_fn(nyaterm_store::StoreDomain::Settings, |store| {
                    store.load_app_settings_summary()
                })
                .unwrap();
            assert_eq!(stored.window_decorations, WindowDecoration::Client);
            app.cancel_settings(cx);
            assert_eq!(
                app.settings.summary().window_decorations,
                WindowDecoration::Client
            );
            assert!(app.window_decoration_preview().is_none());
            app.begin_settings_draft(cx);
            app.set_window_decorations(WindowDecoration::Server, cx);
            app.apply_settings_draft(false, cx);
        });
        cx.run_until_parked();
        app.update(&mut cx, |app, cx| {
            let stored = app
                .store_blocking_client()
                .request_fn(nyaterm_store::StoreDomain::Settings, |store| {
                    store.load_app_settings_summary()
                })
                .unwrap();
            assert_eq!(stored.window_decorations, WindowDecoration::Server);
            app.cancel_settings(cx);
        });
    }

    #[test]
    fn shared_native_material_settings_refresh_other_workspaces_and_respect_dirty_drafts() {
        use crate::app_shell::{SharedStateDomain, SharedStateEvent};
        use nyaterm_core::models::window_material::WindowMaterial;
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let first = app(&mut cx);
        let second = app(&mut cx);
        let mut snapshot = cx.update_entity(&first, |app, _| {
            app.store_blocking_client()
                .request(0, nyaterm_store::LoadBootstrap)
                .unwrap()
                .outcome
                .unwrap()
        });
        snapshot.settings.window_material = WindowMaterial::Blurred;
        snapshot.settings.window_background_opacity = 48;
        let event = SharedStateEvent {
            domain: SharedStateDomain::Settings,
            revision: 1,
            changed: true,
        };
        cx.update_entity(&second, |app, cx| {
            app.apply_shared_state(snapshot.clone(), event, cx);
            assert_eq!(
                app.settings.summary().window_material,
                WindowMaterial::Blurred
            );
            assert_eq!(app.settings.summary().window_background_opacity, 48);
            app.begin_settings_draft(cx);
            app.set_window_material(WindowMaterial::Transparent, cx);
            app.apply_shared_state(snapshot.clone(), event, cx);
            assert_eq!(
                app.settings.summary().window_material,
                WindowMaterial::Transparent
            );
            app.cancel_settings(cx);
            assert_eq!(
                app.settings.summary().window_material,
                WindowMaterial::Blurred
            );
        });
    }

    /// And must write a header status turned back on from hidden.
    ///
    /// This is the direction the bug was reported in: the header reads as "hidden"
    /// again the moment settings are applied. Seeded through the store so the stale
    /// on-disk value is genuinely `false` rather than the `true` default, which is what
    /// makes the revert visible at all.
    #[test]
    fn applying_a_draft_persists_a_header_status_turned_back_on() {
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx);
        cx.update_entity(&app, |app, cx| {
            // No draft open, so this takes the immediate-persist path.
            app.set_header_status_visible(false, cx);
            app.queue_settings_save(SettingsSaveKind::UiLayout, cx);
        });
        cx.run_until_parked();
        assert_eq!(
            stored_header_status(&app, &mut cx),
            ("session".to_string(), false),
            "the seed must reach disk, or there is no stale value to revert to"
        );

        cx.update_entity(&app, |app, cx| {
            app.begin_settings_draft(cx);
            app.set_header_status_mode(HeaderStatusMode::Host, cx);
            app.apply_settings_draft(false, cx);
        });
        cx.run_until_parked();

        assert_eq!(
            stored_header_status(&app, &mut cx),
            ("host".to_string(), true),
            "apply must write the header back on, not leave it hidden on disk"
        );
    }

    #[test]
    fn completing_host_tool_slots_does_not_mask_concurrent_visibility_or_placement_changes() {
        for move_left in [false, true] {
            let mut cx = TestAppContext::single();
            let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
            let app = app(&mut cx);
            let (store, mut external) = cx.update_entity(&app, |app, cx| {
                app.begin_settings_draft(cx);
                app.apply_interaction_word_separators("fixture changed separators".into(), cx);
                (
                    app.store_blocking_client().clone(),
                    app.settings.summary().clone(),
                )
            });
            if move_left {
                external
                    .ui_activity_bar_right_top
                    .retain(|id| id != "portsManager");
                external
                    .ui_activity_bar_left_top
                    .push("portsManager".into());
            } else {
                external
                    .ui_activity_bar_hidden_items
                    .retain(|id| id != "portsManager");
            }
            let original_separators = store
                .request_fn(nyaterm_store::StoreDomain::Settings, |store| {
                    store.load_app_settings_summary()
                })
                .unwrap()
                .interaction_word_separators;
            let expected = external.clone();
            store
                .request_fn(nyaterm_store::StoreDomain::Settings, move |store| {
                    store.save_ui_layout_settings(&external)
                })
                .unwrap();
            cx.update_entity(&app, |app, cx| app.apply_settings_draft(false, cx));
            cx.run_until_parked();
            let saved = store
                .request_fn(nyaterm_store::StoreDomain::Settings, |store| {
                    store.load_app_settings_summary()
                })
                .unwrap();
            assert_eq!(saved.interaction_word_separators, original_separators);
            assert_eq!(
                saved.ui_activity_bar_left_top,
                expected.ui_activity_bar_left_top
            );
            assert_eq!(
                saved.ui_activity_bar_hidden_items,
                expected.ui_activity_bar_hidden_items
            );
            cx.update_entity(&app, |app, _| assert!(app.shell.has_settings_draft()));
        }
    }

    #[test]
    fn stale_draft_cannot_overwrite_settings_saved_by_another_window() {
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx);
        cx.update_entity(&app, |app, cx| {
            app.begin_settings_draft(cx);
            app.set_header_status_mode(HeaderStatusMode::Host, cx);
            let mut other_window = app.settings.summary().clone();
            other_window.ui_header_status_mode = "session".to_string();
            other_window.confirm_on_close = !other_window.confirm_on_close;
            app.store_blocking_client()
                .request_fn(nyaterm_store::StoreDomain::Settings, move |store| {
                    store.save_general_settings(&other_window)
                })
                .expect("external save");
            app.apply_settings_draft(false, cx);
        });
        cx.run_until_parked();
        assert_eq!(stored_header_status(&app, &mut cx).0, "session");
        cx.update_entity(&app, |app, _| {
            assert!(
                app.shell.has_settings_draft(),
                "the rejected draft must survive"
            );
        });
    }

    #[test]
    fn applying_unrelated_settings_does_not_rewrite_updated_keyword_catalog() {
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx);
        cx.update_entity(&app, |app, cx| {
            app.begin_settings_draft(cx);
            app.set_header_status_mode(HeaderStatusMode::Host, cx);
            let mut external = app.settings.keyword_config().clone();
            external.enabled = !external.enabled;
            app.store_blocking_client()
                .request_fn(nyaterm_store::StoreDomain::Settings, move |store| {
                    store.save_keyword_highlights(&external)
                })
                .expect("external keyword save");
            app.apply_settings_draft(false, cx);
        });
        cx.run_until_parked();
        let saved = cx.update_entity(&app, |app, _| {
            app.store_blocking_client()
                .request_fn(nyaterm_store::StoreDomain::Settings, |store| {
                    store.load_keyword_highlights()
                })
                .expect("load keyword catalog")
        });
        assert!(saved.enabled);
        assert_eq!(stored_header_status(&app, &mut cx).0, "host");
    }

    #[test]
    fn cloud_sync_without_a_master_password_can_be_disabled_and_reenabled_after_recovery() {
        for same_draft in [false, true] {
            let mut cx = TestAppContext::single();
            let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
            let app = app(&mut cx);
            app.update(&mut cx, |app, cx| {
                let cloud = app
                    .store_blocking_client()
                    .request_fn(nyaterm_store::StoreDomain::CloudSync, |store| {
                        let mut cloud = nyaterm_core::CloudSyncSettings {
                            enabled: true,
                            ..Default::default()
                        };
                        cloud.webdav.endpoint = "https://dav.example.invalid".into();
                        store.save_cloud_sync_settings(cloud)
                    })
                    .unwrap();
                app.cloud_sync.replace_loaded(cloud, Default::default());
                assert!(!app.settings.summary().has_master_password);
                assert!(!app.cloud_sync_form_enabled());
                app.begin_settings_draft(cx);
                assert!(app.pending_settings_validation_error().is_some());
                app.toggle_cloud_sync_enabled(cx);
                assert!(!app.cloud_sync.settings().enabled);
                assert!(app.pending_settings_validation_error().is_none());

                // Cancelling a recovery draft must leave the committed settings intact.
                app.cancel_settings(cx);
                assert!(app.cloud_sync.settings().enabled);
                assert!(!app.settings.master_password().enabled);
                app.begin_settings_draft(cx);
                app.toggle_cloud_sync_enabled(cx);
                // The enable guard must still reject a missing password.
                app.toggle_cloud_sync_enabled(cx);
                assert!(!app.cloud_sync.settings().enabled);
                if same_draft {
                    app.toggle_settings_master_password(cx);
                    app.apply_settings_master_password("fixture-recovery-password".into(), cx);
                }
                app.apply_settings_draft(false, cx);
            });
            finish_settings_apply(&app, &mut cx);
            let (settings, cloud) = stored_settings_and_cloud(&app, &mut cx);
            assert!(!cloud.enabled);
            assert_eq!(settings.has_master_password, same_draft);
            if !same_draft {
                app.update(&mut cx, |app, cx| {
                    app.toggle_settings_master_password(cx);
                    app.apply_settings_master_password("fixture-recovery-password".into(), cx);
                    app.apply_settings_draft(false, cx);
                });
                finish_settings_apply(&app, &mut cx);
            }
            app.update(&mut cx, |app, cx| {
                assert!(app.settings.summary().has_master_password);
                assert!(app.cloud_sync_form_enabled());
                app.toggle_cloud_sync_enabled(cx);
                assert!(app.cloud_sync.settings().enabled);
                app.confirm_settings_draft(cx);
            });
            finish_settings_apply(&app, &mut cx);
            let (settings, cloud) = stored_settings_and_cloud(&app, &mut cx);
            assert!(settings.has_master_password);
            assert!(cloud.enabled);
            assert_eq!(cloud.webdav.endpoint, "https://dav.example.invalid");
        }
    }

    #[test]
    fn enabling_master_password_without_a_password_blocks_apply() {
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx);
        let draft_survived = cx.update_entity(&app, |app, cx| {
            app.begin_settings_draft(cx);
            app.settings
                .toggle_master_password(false)
                .expect("enable staged");
            assert!(app.settings.master_password().enabled, "switch is on");

            app.apply_settings_draft(false, cx);

            assert!(
                app.settings.master_password().enabled,
                "apply must not silently turn the master password switch off"
            );
            assert!(
                app.shell.status().contains("master password"),
                "the block must report why settings did not apply"
            );
            app.shell.has_settings_draft()
        });
        assert!(draft_survived, "the rejected draft must survive");
        let summary = cx.update_entity(&app, |app, _| {
            app.store_blocking_client()
                .request_fn(nyaterm_store::StoreDomain::Settings, |store| {
                    store.load_app_settings_summary()
                })
                .expect("load stored settings")
        });
        assert!(
            !summary.has_master_password,
            "nothing may reach disk while the draft is blocked"
        );
    }

    #[test]
    fn enabling_master_password_with_a_password_persists() {
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx);
        cx.update_entity(&app, |app, cx| {
            app.begin_settings_draft(cx);
            app.settings
                .toggle_master_password(false)
                .expect("enable staged");
            assert!(
                app.settings
                    .edit_master_password_draft("staged secret".to_string()),
                "the draft is editable while enabled"
            );
            app.apply_settings_draft(false, cx);
        });
        cx.run_until_parked();
        let summary = cx.update_entity(&app, |app, _| {
            app.store_blocking_client()
                .request_fn(nyaterm_store::StoreDomain::Settings, |store| {
                    store.load_app_settings_summary()
                })
                .expect("load stored settings")
        });
        assert!(
            summary.has_master_password,
            "apply must persist the typed master password"
        );
    }

    #[test]
    fn webdav_edits_apply_with_a_persisted_master_password() {
        use crate::models::CloudSyncInputField;

        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx);
        cx.update_entity(&app, |app, cx| {
            app.begin_settings_draft(cx);
            app.settings
                .toggle_master_password(false)
                .expect("enable staged");
            assert!(app.settings.edit_master_password_draft("msk".to_string()));
            app.apply_settings_draft(false, cx);
        });
        cx.run_until_parked();
        let persisted = cx.update_entity(&app, |app, _| {
            app.store_blocking_client()
                .request_fn(nyaterm_store::StoreDomain::Settings, |store| {
                    store.load_app_settings_summary()
                })
                .expect("load stored settings")
        });
        assert!(
            persisted.has_master_password,
            "the password must reach disk"
        );
        cx.update_entity(&app, |app, cx| {
            // Replicate the post-apply callback, which the harness leaves undelivered.
            app.settings.replace_summary(persisted.clone());
            app.settings.rebase_master_password();
            assert!(app.settings.master_password().enabled);
            assert!(app.cloud_sync_form_enabled());

            app.begin_settings_draft(cx);
            app.toggle_cloud_sync_enabled(cx);
            app.apply_cloud_sync_input(
                CloudSyncInputField::WebdavEndpoint,
                "https://dav.example.com".to_string(),
                cx,
            );
            assert!(
                app.settings_draft_dirty(),
                "the webdav edit must mark the draft dirty"
            );
            assert_eq!(
                app.pending_settings_validation_error(),
                None,
                "no reason to block a valid webdav apply"
            );
            app.apply_settings_draft(false, cx);
        });
        cx.run_until_parked();
        let cloud = cx.update_entity(&app, |app, _| {
            app.store_blocking_client()
                .request_fn(nyaterm_store::StoreDomain::Settings, |store| {
                    store.load_cloud_sync_settings()
                })
                .expect("load cloud sync settings")
        });
        assert_eq!(cloud.webdav.endpoint, "https://dav.example.com");
        assert!(cloud.enabled);
    }

    /// The exact state behind "WebDAV edits never reach the apply button": the
    /// master password switch is staged on but no password is stored and none is
    /// typed, so the cloud form is not enabled. The field edits must still land in
    /// the draft -- otherwise the draft stays clean, the apply button stays grey and
    /// no message explains the block -- and apply must report the missing password.
    #[test]
    fn webdav_edits_with_a_staged_master_password_report_the_missing_password() {
        use crate::models::CloudSyncInputField;

        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx);
        let draft_survived = cx.update_entity(&app, |app, cx| {
            app.begin_settings_draft(cx);
            app.settings
                .toggle_master_password(false)
                .expect("enable staged");
            assert!(
                app.settings.master_password().enabled,
                "the switch is on without a stored password"
            );
            assert!(
                !app.cloud_sync_form_enabled(),
                "the reported state: on-disk has no master password"
            );

            app.apply_cloud_sync_input(
                CloudSyncInputField::WebdavEndpoint,
                "https://dav.example.com".to_string(),
                cx,
            );
            assert_eq!(
                app.cloud_sync.settings().webdav.endpoint,
                "https://dav.example.com",
                "the field edit must land in the draft"
            );
            assert!(
                app.settings_draft_dirty(),
                "the webdav edit must mark the draft dirty"
            );
            assert!(
                app.pending_settings_validation_error()
                    .is_some_and(|message| !message.trim().is_empty()),
                "the block must explain itself once the draft is dirty"
            );
            app.apply_settings_draft(false, cx);
            app.shell.has_settings_draft()
        });
        assert!(draft_survived, "the blocked draft must survive");
        let cloud = cx.update_entity(&app, |app, _| {
            app.store_blocking_client()
                .request_fn(nyaterm_store::StoreDomain::Settings, |store| {
                    store.load_cloud_sync_settings()
                })
                .expect("load cloud sync settings")
        });
        assert_eq!(
            cloud.webdav.endpoint, "",
            "the blocked draft must not reach disk"
        );
    }

    /// A provider config can be prepared before any master password exists: cloud
    /// sync stays disabled, but the endpoint edit persists when applied.
    #[test]
    fn webdav_edits_persist_without_a_master_password_while_cloud_stays_disabled() {
        use crate::models::CloudSyncInputField;

        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx);
        cx.update_entity(&app, |app, cx| {
            app.begin_settings_draft(cx);
            assert!(
                !app.settings.master_password().enabled,
                "fixture: no master password set up at all"
            );
            app.apply_cloud_sync_input(
                CloudSyncInputField::WebdavEndpoint,
                "https://dav.example.com".to_string(),
                cx,
            );
            assert!(
                app.settings_draft_dirty(),
                "the webdav edit must mark the draft dirty"
            );
            assert_eq!(
                app.pending_settings_validation_error(),
                None,
                "cloud sync is not enabled, so no validation blocks a config save"
            );
            app.apply_settings_draft(false, cx);
        });
        cx.run_until_parked();
        let cloud = cx.update_entity(&app, |app, _| {
            app.store_blocking_client()
                .request_fn(nyaterm_store::StoreDomain::Settings, |store| {
                    store.load_cloud_sync_settings()
                })
                .expect("load cloud sync settings")
        });
        assert_eq!(cloud.webdav.endpoint, "https://dav.example.com");
        assert!(!cloud.enabled, "cloud sync must stay disabled");
    }

    /// The apply button reads the panel's flushed snapshot, not the draft directly.
    /// A webdav edit must make that snapshot dirty so the button stops being grey
    /// once the draft is valid -- and must keep it blocked with a visible reason
    /// while a master password is staged without a stored one.
    #[test]
    fn flushed_panel_snapshot_reports_webdav_edits() {
        use crate::models::CloudSyncInputField;

        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx);
        let panel = cx.update_entity(&app, |app, cx| {
            app.begin_settings_draft(cx);
            app.settings
                .toggle_master_password(false)
                .expect("enable staged");
            app.apply_cloud_sync_input(
                CloudSyncInputField::WebdavEndpoint,
                "https://dav.example.com".to_string(),
                cx,
            );
            // The main window's settings view requests a panel refresh on every
            // render; flushing is how that reaches the panel, so drive it directly.
            app.flush_settings_panel_snapshots(cx);
            app.settings_panel.clone()
        });
        let snapshot = cx
            .update_entity(&panel, |panel, _| panel.snapshot().cloned())
            .expect("the test app builds the settings panel");
        assert!(
            snapshot.draft_dirty,
            "the flushed snapshot must report the webdav edit as dirty"
        );
        assert!(
            snapshot.validation_error.is_some(),
            "a staged master password with no stored one blocks apply, and the \
             reason must reach the panel"
        );
    }
    fn finish_theme_jobs(app: &gpui::Entity<NyaTermApp>, cx: &mut TestAppContext) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            cx.run_until_parked();
            if !cx.update_entity(app, |app, _| app.settings.theme_designer().submitting) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "theme commit did not complete"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        cx.run_until_parked();
    }

    fn stage_invalid_cloud_draft(app: &mut NyaTermApp, cx: &mut gpui::Context<NyaTermApp>) {
        app.settings
            .toggle_master_password(false)
            .expect("stage master password");
        assert!(
            app.settings
                .edit_master_password_draft("fixture password".into())
        );
        app.cloud_sync.toggle_enabled();
        app.apply_cloud_sync_input(
            crate::models::CloudSyncInputField::WebdavEndpoint,
            "   ".into(),
            cx,
        );
        assert!(app.settings_draft_dirty());
        assert!(app.pending_settings_validation_error().is_some());
    }

    fn stored_settings_and_cloud(
        app: &gpui::Entity<NyaTermApp>,
        cx: &mut TestAppContext,
    ) -> (
        nyaterm_core::AppSettingsSummary,
        nyaterm_core::CloudSyncSettings,
    ) {
        app.update(cx, |app, _| {
            app.store_blocking_client()
                .request_fn(nyaterm_store::StoreDomain::Settings, |store| {
                    Ok((
                        store.load_app_settings_summary()?,
                        store.load_cloud_sync_settings()?,
                    ))
                })
                .expect("load saved settings and cloud configuration")
        })
    }

    fn finish_settings_apply(app: &gpui::Entity<NyaTermApp>, cx: &mut TestAppContext) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            cx.run_until_parked();
            if app.read_with(cx, |app, _| {
                app.settings.store_status().message == rust_i18n::t!("settings.applied")
            }) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "settings apply did not complete"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    #[test]
    fn invalid_settings_actions_preserve_both_drafts_without_starting_a_theme_save() {
        use crate::features::settings::theme_designer::ThemeIntent;

        for close in [false, true] {
            for edit_theme in [false, true] {
                let mut cx = TestAppContext::single();
                let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
                let app = app(&mut cx);
                app.update(&mut cx, |app, cx| {
                    app.open_page(crate::models::NavItem::Settings, cx);
                    if edit_theme {
                        app.theme_designer_action(ThemeIntent::Copy("dracula".into()), cx);
                        assert!(app.save_theme_designer(None, cx));
                    }
                });
                finish_theme_jobs(&app, &mut cx);
                let (original_settings, original_cloud) = stored_settings_and_cloud(&app, &mut cx);
                let (theme_draft, error) = app.update(&mut cx, |app, cx| {
                    if edit_theme {
                        app.edit_theme_designer("primary", "#123456".into(), cx);
                    }
                    stage_invalid_cloud_draft(app, cx);
                    let theme_draft = app.settings.theme_designer().draft.clone();
                    let error = app.pending_settings_validation_error().unwrap();
                    if close {
                        app.confirm_settings_draft(cx);
                    } else {
                        app.apply_settings_draft(false, cx);
                    }
                    assert_eq!(app.settings.store_status().message, error);
                    assert!(app.shell.status().contains(&error));
                    assert!(app.settings.theme_designer().pending.is_none());
                    assert!(!app.settings.theme_designer().submitting);
                    (theme_draft, error)
                });
                cx.run_until_parked();
                let (saved_settings, saved_cloud) = stored_settings_and_cloud(&app, &mut cx);
                assert_eq!(saved_settings, original_settings);
                assert!(
                    saved_cloud == original_cloud,
                    "cloud configuration must not be saved"
                );
                app.read_with(&cx, |app, _| {
                    assert!(app.settings_tab_is_open());
                    assert!(app.shell.has_settings_draft());
                    assert!(app.settings_draft_dirty());
                    assert_eq!(app.cloud_sync.settings().webdav.endpoint, "   ");
                    assert!(app.settings.master_password().enabled);
                    assert!(!app.settings.master_password().draft.is_empty());
                    assert!(app.settings.theme_designer().draft == theme_draft);
                    assert_eq!(app.settings.theme_designer().dirty(), edit_theme);
                    assert_eq!(app.pending_settings_validation_error(), Some(error));
                });
            }
        }
    }

    #[test]
    fn corrected_cloud_drafts_apply_and_confirm_successfully() {
        for close in [false, true] {
            let mut cx = TestAppContext::single();
            let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
            let app = app(&mut cx);
            app.update(&mut cx, |app, cx| {
                app.open_page(crate::models::NavItem::Settings, cx);
                stage_invalid_cloud_draft(app, cx);
                app.confirm_settings_draft(cx);
                assert!(app.settings_tab_is_open());
                app.apply_cloud_sync_input(
                    crate::models::CloudSyncInputField::WebdavEndpoint,
                    "https://dav.example.com".into(),
                    cx,
                );
                assert!(app.pending_settings_validation_error().is_none());
                if close {
                    app.confirm_settings_draft(cx);
                } else {
                    app.apply_settings_draft(false, cx);
                }
            });
            finish_settings_apply(&app, &mut cx);
            let (saved, cloud) = stored_settings_and_cloud(&app, &mut cx);
            assert!(saved.has_master_password);
            assert!(cloud.enabled);
            assert_eq!(cloud.webdav.endpoint, "https://dav.example.com");
            app.read_with(&cx, |app, _| {
                assert_eq!(app.settings_tab_is_open(), !close);
                assert_eq!(app.shell.has_settings_draft(), !close);
                assert!(!app.settings_draft_dirty());
            });
        }
    }

    #[test]
    fn theme_save_resumes_valid_apply_and_confirm_intents() {
        use crate::features::settings::theme_designer::ThemeIntent;

        for close in [false, true] {
            let mut cx = TestAppContext::single();
            let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
            let app = app(&mut cx);
            app.update(&mut cx, |app, cx| {
                app.open_page(crate::models::NavItem::Settings, cx);
                app.set_header_status_mode(HeaderStatusMode::Host, cx);
                app.theme_designer_action(ThemeIntent::Copy("dracula".into()), cx);
                app.edit_theme_designer("primary", "#123456".into(), cx);
                if close {
                    app.confirm_settings_draft(cx);
                } else {
                    app.apply_settings_draft(false, cx);
                }
                assert_eq!(
                    app.settings.theme_designer().pending,
                    Some(ThemeIntent::Apply(close))
                );
                app.resolve_theme_designer_pending(Some(true), cx);
            });
            finish_theme_jobs(&app, &mut cx);
            finish_settings_apply(&app, &mut cx);
            let (saved, _) = stored_settings_and_cloud(&app, &mut cx);
            assert_eq!(saved.ui_header_status_mode, "host");
            assert_eq!(saved.custom_themes.available()[0].colors.primary, "#123456");
            app.read_with(&cx, |app, _| {
                assert_eq!(app.settings_tab_is_open(), !close);
                assert!(!app.settings_draft_dirty());
                assert!(!app.settings.theme_designer().dirty());
                assert!(app.settings.theme_designer().pending.is_none());
                assert!(!app.settings.theme_designer().submitting);
            });
        }
    }

    #[test]
    fn theme_only_actions_do_not_validate_unchanged_ordinary_settings() {
        use crate::features::settings::theme_designer::ThemeIntent;

        for invalid_baseline in [false, true] {
            for close in [false, true] {
                let mut cx = TestAppContext::single();
                let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
                let app = app(&mut cx);
                app.update(&mut cx, |app, cx| {
                    if invalid_baseline {
                        app.cloud_sync.toggle_enabled();
                    }
                    app.open_page(crate::models::NavItem::Settings, cx);
                    assert!(!app.settings_draft_dirty());
                    assert_eq!(
                        app.pending_settings_validation_error().is_some(),
                        invalid_baseline
                    );
                    app.theme_designer_action(ThemeIntent::Copy("dracula".into()), cx);
                    if close {
                        app.confirm_settings_draft(cx);
                    } else {
                        app.apply_settings_draft(false, cx);
                    }
                    assert_eq!(
                        app.settings.theme_designer().pending,
                        Some(ThemeIntent::Apply(close))
                    );
                    app.resolve_theme_designer_pending(Some(true), cx);
                });
                finish_theme_jobs(&app, &mut cx);
                finish_settings_apply(&app, &mut cx);
                let (saved, cloud) = stored_settings_and_cloud(&app, &mut cx);
                assert_eq!(saved.custom_themes.available().len(), 1);
                assert!(
                    !cloud.enabled,
                    "unchanged cloud configuration must not be persisted"
                );
                app.read_with(&cx, |app, _| {
                    assert_eq!(app.settings_tab_is_open(), !close);
                    assert!(!app.settings.theme_designer().dirty());
                });
            }
        }
    }

    #[test]
    fn explicit_theme_save_still_succeeds_with_an_invalid_ordinary_draft() {
        use crate::features::settings::theme_designer::ThemeIntent;

        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx);
        let (original, original_cloud) = stored_settings_and_cloud(&app, &mut cx);
        app.update(&mut cx, |app, cx| {
            app.open_page(crate::models::NavItem::Settings, cx);
            stage_invalid_cloud_draft(app, cx);
            app.theme_designer_action(ThemeIntent::Copy("dracula".into()), cx);
            assert!(app.save_theme_designer(None, cx));
        });
        finish_theme_jobs(&app, &mut cx);
        let (saved, cloud) = stored_settings_and_cloud(&app, &mut cx);
        assert_eq!(saved.custom_themes.available().len(), 1);
        assert_eq!(saved.has_master_password, original.has_master_password);
        assert!(cloud == original_cloud);
        app.read_with(&cx, |app, _| {
            assert!(app.settings_tab_is_open());
            assert!(app.settings_draft_dirty());
            assert!(app.pending_settings_validation_error().is_some());
            assert!(!app.settings.theme_designer().dirty());
        });
    }

    #[test]
    fn theme_browsing_is_local_and_saved_themes_survive_settings_cancel() {
        use crate::features::settings::theme_designer::ThemeIntent;
        use nyaterm_core::themes::management::ThemeApplyTarget;
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx);
        let original = cx.update_entity(&app, |app, cx| {
            let original = app.settings.summary().clone();
            app.begin_settings_draft(cx);
            app.settings.set_terminal_font_size(27);
            app.theme_designer_action(ThemeIntent::Toggle, cx);
            assert!(!app.settings.theme_designer().dirty());
            app.theme_designer_action(ThemeIntent::Select("dracula".into()), cx);
            assert_eq!(app.settings.summary().theme, original.theme);
            app.theme_designer_action(ThemeIntent::Copy("dracula".into()), cx);
            app.edit_theme_designer("primary", "#123456".into(), cx);
            assert!(app.save_theme_designer(Some(ThemeApplyTarget::BothFollowUi), cx));
            assert_eq!(app.settings.summary().theme, original.theme);
            original
        });
        finish_theme_jobs(&app, &mut cx);
        let id = cx.update_entity(&app, |app, cx| {
            let id = app.settings.summary().theme.clone();
            assert_eq!(app.theme_palette().primary, 0x123456);
            assert_eq!(app.settings.summary().terminal_theme, None);
            assert_eq!(app.settings.summary().terminal_font_size, 27);
            assert!(app.settings_draft_dirty());
            app.cancel_settings(cx);
            assert_eq!(app.settings.summary().theme, id);
            assert_eq!(
                app.settings.summary().terminal_font_size,
                original.terminal_font_size
            );
            id
        });
        let saved = cx.update_entity(&app, |app, _| {
            app.store_blocking_client()
                .request_fn(nyaterm_store::StoreDomain::Settings, |store| {
                    store.load_app_settings_summary()
                })
                .unwrap()
        });
        assert_eq!(saved.theme, id);
        assert_eq!(saved.terminal_font_size, original.terminal_font_size);
        assert_eq!(
            saved.custom_themes.resolve(&id).unwrap().colors.primary,
            "#123456"
        );
    }

    #[test]
    fn saving_an_active_theme_refreshes_it_without_committing_other_settings() {
        use crate::features::settings::theme_designer::ThemeIntent;
        use nyaterm_core::themes::management::ThemeApplyTarget;
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx);
        cx.update_entity(&app, |app, cx| {
            app.begin_settings_draft(cx);
            app.theme_designer_action(ThemeIntent::Copy("github-dark".into()), cx);
            assert!(app.save_theme_designer(Some(ThemeApplyTarget::Ui), cx));
        });
        finish_theme_jobs(&app, &mut cx);
        cx.update_entity(&app, |app, cx| {
            app.edit_theme_designer("primary", "#abcdef".into(), cx);
            assert!(app.save_theme_designer(None, cx));
        });
        finish_theme_jobs(&app, &mut cx);
        cx.update_entity(&app, |app, cx| {
            assert_eq!(app.theme_palette().primary, 0xabcdef);
            assert_eq!(app.terminal_theme_palette().primary, 0xabcdef);
            assert!(!app.settings_draft_dirty());
            app.settings.set_terminal_font_size(29);
            app.save_appearance_settings(cx);
            app.apply_settings_draft(false, cx);
        });
        cx.run_until_parked();
        let saved = cx.update_entity(&app, |app, _| {
            app.store_blocking_client()
                .request_fn(nyaterm_store::StoreDomain::Settings, |store| {
                    store.load_app_settings_summary()
                })
                .unwrap()
        });
        assert_eq!(saved.terminal_font_size, 29);
        assert_eq!(
            saved
                .custom_themes
                .resolve(&saved.theme)
                .unwrap()
                .colors
                .primary,
            "#abcdef"
        );
    }

    #[test]
    fn theme_conflicts_preserve_edits_and_do_not_apply_the_theme() {
        use crate::features::settings::theme_designer::ThemeIntent;
        use nyaterm_core::themes::management::ThemeApplyTarget;
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx);
        cx.update_entity(&app, |app, cx| {
            app.begin_settings_draft(cx);
            app.theme_designer_action(ThemeIntent::Copy("github-dark".into()), cx);
            app.edit_theme_designer("primary", "#123456".into(), cx);
            app.store_blocking_client()
                .request_fn(nyaterm_store::StoreDomain::Settings, |store| {
                    let mut settings = store.load_app_settings_summary()?;
                    settings.theme = "dracula".into();
                    store.save_appearance_settings(&settings)
                })
                .unwrap();
            assert!(app.save_theme_designer(Some(ThemeApplyTarget::Ui), cx));
            assert!(!app.save_theme_designer(Some(ThemeApplyTarget::Ui), cx));
            app.theme_designer_action(ThemeIntent::Toggle, cx);
            assert!(app.settings.theme_designer().open);
        });
        finish_theme_jobs(&app, &mut cx);
        cx.update_entity(&app, |app, _| {
            assert!(app.settings.theme_designer().dirty());
            assert_eq!(
                app.settings
                    .theme_designer()
                    .draft
                    .as_ref()
                    .unwrap()
                    .colors
                    .primary,
                "#123456"
            );
            assert!(!app.settings.theme_designer().message.is_empty());
            assert!(app.settings.summary().custom_themes.available().is_empty());
        });
    }

    #[test]
    fn unsaved_theme_transition_can_continue_discard_or_save_before_leaving() {
        use crate::features::settings::theme_designer::ThemeIntent;
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx);
        cx.update_entity(&app, |app, cx| {
            app.begin_settings_draft(cx);
            app.theme_designer_action(ThemeIntent::Copy("github-dark".into()), cx);
            app.theme_designer_action(ThemeIntent::Toggle, cx);
            assert_eq!(
                app.settings.theme_designer().pending,
                Some(ThemeIntent::Toggle)
            );
            app.resolve_theme_designer_pending(None, cx);
            assert!(app.settings.theme_designer().dirty());
            app.theme_designer_action(ThemeIntent::Toggle, cx);
            app.resolve_theme_designer_pending(Some(false), cx);
            assert!(!app.settings.theme_designer().open);
            app.theme_designer_action(ThemeIntent::Copy("github-dark".into()), cx);
            app.edit_theme_designer("primary", "#abcdef".into(), cx);
            app.theme_designer_action(ThemeIntent::Toggle, cx);
            app.resolve_theme_designer_pending(Some(true), cx);
            assert!(app.settings.theme_designer().open);
        });
        finish_theme_jobs(&app, &mut cx);
        cx.update_entity(&app, |app, _| {
            assert!(!app.settings.theme_designer().open);
            assert_eq!(
                app.settings.summary().custom_themes.available()[0]
                    .colors
                    .primary,
                "#abcdef"
            );
        });
    }

    #[test]
    fn imports_are_unsaved_and_stale_import_results_cannot_replace_edits() {
        use crate::features::settings::theme_designer::ThemeIntent;
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx);
        cx.update_entity(&app, |app, cx| {
            app.begin_settings_draft(cx);
            app.theme_designer_action(ThemeIntent::Copy("github-dark".into()), cx);
            let generation = app.settings.theme_designer().generation;
            app.edit_theme_designer("primary", "#123456".into(), cx);
            let imported = nyaterm_core::themes::builtins()[1].copy_as_custom();
            app.finish_theme_import(generation, Ok(Some(imported.clone())), cx);
            assert_eq!(
                app.settings
                    .theme_designer()
                    .draft
                    .as_ref()
                    .unwrap()
                    .colors
                    .primary,
                "#123456"
            );
            let generation = app.settings.theme_designer().generation;
            app.finish_theme_import(generation, Ok(Some(imported)), cx);
            assert!(app.settings.theme_designer().dirty());
            assert!(app.settings.summary().custom_themes.available().is_empty());
            app.cancel_settings(cx);
            assert_eq!(
                app.settings.theme_designer().pending,
                Some(ThemeIntent::CloseSettings)
            );
        });
    }

    #[test]
    fn deleting_an_active_theme_requires_confirmation_and_preserves_unrelated_drafts() {
        use crate::features::settings::theme_designer::ThemeIntent;
        use nyaterm_core::themes::management::ThemeApplyTarget;
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx);
        cx.update_entity(&app, |app, cx| {
            app.begin_settings_draft(cx);
            app.theme_designer_action(ThemeIntent::Copy("github-dark".into()), cx);
            assert!(app.save_theme_designer(Some(ThemeApplyTarget::BothFollowUi), cx));
        });
        finish_theme_jobs(&app, &mut cx);
        cx.update_entity(&app, |app, cx| {
            let id = app.settings.summary().theme.clone();
            app.settings.set_terminal_font_size(28);
            app.theme_designer_action(ThemeIntent::Delete, cx);
            assert!(app.settings.theme_designer().delete_confirmation);
            assert!(app.settings.summary().custom_themes.resolve(&id).is_some());
            app.settings.theme_designer_mut().delete_confirmation = false;
            assert!(app.settings.summary().custom_themes.resolve(&id).is_some());
            app.theme_designer_action(ThemeIntent::Delete, cx);
            app.confirm_theme_delete(cx);
        });
        finish_theme_jobs(&app, &mut cx);
        cx.update_entity(&app, |app, cx| {
            assert_eq!(app.settings.summary().theme, "github-dark");
            assert_eq!(app.settings.summary().terminal_theme, None);
            assert!(app.settings.summary().custom_themes.available().is_empty());
            assert_eq!(app.settings.summary().terminal_font_size, 28);
            app.cancel_settings(cx);
            assert!(app.settings.summary().custom_themes.available().is_empty());
        });
    }

    /// Write settings straight into the store, the way a cloud pull does.
    fn pull_terminal_scrollback_into_store(
        app: &gpui::Entity<NyaTermApp>,
        cx: &mut TestAppContext,
    ) {
        cx.update_entity(app, |app, _| {
            app.store_blocking_client()
                .request_fn(nyaterm_store::StoreDomain::Settings, |store| {
                    let mut settings = store.load_app_settings_summary()?;
                    settings.terminal_scrollback_lines = 4242;
                    store.save_terminal_settings(&settings)
                })
                .unwrap();
        });
    }

    /// Store replies are delivered from another thread, so `run_until_parked`
    /// alone does not wait for them (`applying_a_draft_persists_the_header_status_mode`).
    fn wait_for_the_pulled_settings(app: &gpui::Entity<NyaTermApp>, cx: &mut TestAppContext) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            cx.run_until_parked();
            if cx.update_entity(app, |app, _| {
                app.settings.summary().terminal_scrollback_lines == 4242
            }) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the store refresh never reached the in-memory settings"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    #[test]
    fn store_refresh_after_a_pull_rebases_an_untouched_settings_draft() {
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx);
        cx.update_entity(&app, |app, cx| {
            app.begin_settings_draft(cx);
            assert!(!app.settings_draft_dirty());
        });
        pull_terminal_scrollback_into_store(&app, &mut cx);
        cx.update_entity(&app, |app, cx| {
            app.refresh_store_from_runtime_and_sync_theme(cx)
        });
        wait_for_the_pulled_settings(&app, &mut cx);
        cx.update_entity(&app, |app, _| {
            assert!(
                !app.settings_draft_dirty(),
                "a pull must not turn an untouched draft into unsaved edits"
            );
        });
    }

    #[test]
    fn store_refresh_keeps_a_draft_the_user_actually_edited() {
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx);
        cx.update_entity(&app, |app, cx| {
            app.begin_settings_draft(cx);
            app.set_transfer_internal_editor_font_size(19, cx);
            assert!(app.settings_draft_dirty());
        });
        pull_terminal_scrollback_into_store(&app, &mut cx);
        cx.update_entity(&app, |app, cx| {
            app.refresh_store_from_runtime_and_sync_theme(cx)
        });
        wait_for_the_pulled_settings(&app, &mut cx);
        cx.update_entity(&app, |app, _| {
            assert!(
                app.settings_draft_dirty(),
                "refreshing must not silently drop the user's edit"
            );
        });
    }

    /// Opening another panel must not make an untouched settings draft dirty.
    ///
    /// Navigation writes the active panel, its collapse state, the open-panel
    /// stack and the dock sizes into the live summary through `persist_ui_layout`.
    /// A draft compares against the clone taken when it opened, so a wholesale
    /// comparison treats that navigation as an unsaved setting edit and the tab
    /// close starts asking to discard changes.
    #[test]
    fn switching_panels_leaves_an_untouched_settings_draft_clean() {
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx);
        cx.update_entity(&app, |app, cx| {
            app.begin_settings_draft(cx);
            assert!(!app.settings_draft_dirty());

            app.open_page(crate::models::NavItem::Connections, cx);
            assert!(
                !app.settings_draft_dirty(),
                "opening the saved-connections panel must not dirty the draft"
            );

            app.open_page(crate::models::NavItem::Notes, cx);
            assert!(
                !app.settings_draft_dirty(),
                "switching panels must not dirty the draft"
            );

            // The guard has to stay able to see real edits, or a permanently
            // clean draft would pass the assertions above.
            app.set_transfer_internal_editor_font_size(19, cx);
            assert!(
                app.settings_draft_dirty(),
                "a real settings edit must still mark the draft dirty"
            );
        });
    }
    #[test]
    fn cancelling_settings_restores_real_edits_and_preserves_live_navigation() {
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx);
        cx.update_entity(&app, |app, cx| {
            let original_font = app.settings.summary().transfer_internal_editor_font_size;
            app.begin_settings_draft(cx);
            app.open_page(crate::models::NavItem::Notes, cx);
            let live = app.settings.summary().clone();
            app.set_transfer_internal_editor_font_size(19, cx);
            assert!(app.settings_draft_dirty());
            app.cancel_settings(cx);
            assert_eq!(
                app.settings.summary().transfer_internal_editor_font_size,
                original_font
            );
            assert_eq!(
                app.settings.summary().ui_active_left_panel,
                live.ui_active_left_panel
            );
            assert_eq!(
                app.settings.summary().ui_left_open_panels,
                live.ui_left_open_panels
            );
            assert_eq!(
                app.settings.summary().ui_left_panel_width,
                live.ui_left_panel_width
            );
        });
    }

    #[test]
    fn header_and_file_manager_preferences_still_dirty_the_whole_settings_comparison() {
        let baseline = nyaterm_core::AppSettingsSummary::default();
        for field in 0..4 {
            let mut edited = baseline.clone();
            match field {
                0 => edited.ui_header_status_visible = !edited.ui_header_status_visible,
                1 => edited.ui_highlight_line_roles = !edited.ui_highlight_line_roles,
                2 => {
                    edited.ui_file_manager.auto_collapse_sidebar =
                        !edited.ui_file_manager.auto_collapse_sidebar
                }
                _ => {
                    edited.ui_file_manager.remember_layout = !edited.ui_file_manager.remember_layout
                }
            }
            assert!(!super::settings_draft_matches(&baseline, &edited));
        }
    }
}
