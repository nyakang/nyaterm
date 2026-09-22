use rust_i18n::t;

use gpui::Context;
use nyaterm_store::{StoreDomain, store_request};
use nyaterm_transport::SftpDuplicatePolicy;

use crate::features::NyaTermApp;
use crate::features::app_state::SettingsDraftSnapshot;
use crate::models::TranslationSecretDraft;

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
        snapshot.settings != *self.settings.summary()
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
        if !self.shell.has_settings_draft() {
            if close_after_apply {
                self.finish_settings_page(cx);
            }
            self.request_settings_panel_refresh(cx);
            return;
        }
        if let Some(error) = self.pending_settings_validation_error() {
            self.settings.update_store_status(error.clone(), false);
            self.shell
                .set_status(t!("settings.applyBlocked", detail = error));
            cx.notify();
            self.request_settings_panel_refresh(cx);
            return;
        }

        let settings = self.settings.summary().clone();
        let base_settings = self
            .shell
            .settings_draft_snapshot()
            .expect("settings draft checked above")
            .settings
            .clone();
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
        let cloud_sync_settings = self.cloud_sync.pending_settings();
        let translation_settings = self.translation.pending_settings();
        let keyword_highlights = self.settings.keyword_config().clone();
        let master_password_update = if master_password.draft.is_empty() {
            (self.settings.summary().has_master_password && !master_password.enabled)
                .then_some(None)
        } else {
            Some(Some(master_password.draft.to_string()))
        };
        self.settings
            .update_store_status(t!("settings.applying").to_string(), false);
        self.submit_store_request(
            0,
            store_request(StoreDomain::Settings, move |store| {
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
                if cloud_changed && store.load_cloud_sync_settings()? != base.cloud_sync_settings {
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
                    saved_ai_settings,
                ))
            }),
            move |this, event, cx| match event.outcome {
                Ok((
                    saved_settings,
                    saved_keyword_highlights,
                    saved_translation_settings,
                    saved_cloud_sync_settings,
                    saved_ai_settings,
                )) => {
                    this.apply_gpui_settings(saved_settings.clone(), cx);
                    this.publish_shared_settings(saved_settings, cx);
                    this.request_shared_state_refresh(crate::app_shell::SharedStateDomain::All, cx);
                    this.settings.rebase_master_password();
                    this.ai.replace_settings_config(saved_ai_settings, true);
                    this.cloud_sync
                        .replace_settings(saved_cloud_sync_settings, Default::default());
                    this.translation.replace_settings(
                        saved_translation_settings,
                        TranslationSecretDraft::default(),
                    );
                    this.settings
                        .replace_keyword_config(saved_keyword_highlights);
                    this.sync_ai_drafts_from_active_profile();
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
        if let Some(snapshot) = self.shell.take_settings_draft_snapshot() {
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
            self.refresh_visible_terminal_surfaces(cx);
        }
        self.settings.clear_draft_dirty_domains();
        self.finish_settings_page(cx);
        self.request_settings_panel_refresh(cx);
    }

    pub(in crate::features) fn confirm_settings_draft(&mut self, cx: &mut Context<Self>) {
        if self.settings_draft_dirty() {
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
        self.cancel_github_gist_auth(cx);
        self.ai.close_settings_editors();
        self.settings.clear_keyword_highlight_edit();
        self.forget_text_inputs("ai.settings.action.");
        self.forget_text_inputs("ai.settings.manual-model.");
        self.forget_text_inputs("keyword.highlight.");
        if self.shell.finish_settings_navigation() {
            self.persist_ui_layout();
        }
        self.shell
            .set_status(t!("settings.settingsClosed").to_string());
        cx.notify();
    }
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
    use gpui::{AppContext as _, TestAppContext};
    use nyaterm_core::{AppRuntime, RuntimeMode, uuid};

    use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
    use crate::features::NyaTermApp;
    use crate::features::settings::SettingsSaveKind;
    use crate::models::HeaderStatusMode;

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

    #[test]
    fn canceling_a_draft_restores_ui_typography_source() {
        let mut cx = TestAppContext::single();
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

    /// And must write a header status turned back on from hidden.
    ///
    /// This is the direction the bug was reported in: the header reads as "hidden"
    /// again the moment settings are applied. Seeded through the store so the stale
    /// on-disk value is genuinely `false` rather than the `true` default, which is what
    /// makes the revert visible at all.
    #[test]
    fn applying_a_draft_persists_a_header_status_turned_back_on() {
        let mut cx = TestAppContext::single();
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
    fn stale_draft_cannot_overwrite_settings_saved_by_another_window() {
        let mut cx = TestAppContext::single();
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
    fn enabling_master_password_without_a_password_blocks_apply() {
        let mut cx = TestAppContext::single();
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
            assert_eq!(
                app.pending_settings_validation_error(),
                Some("Enter a master password before enabling the master password.".to_string()),
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
}
