use rust_i18n::t;

use gpui::{
    AnyElement, Context, FontWeight, KeyDownEvent, PathPromptOptions, SharedString, Window, div,
    prelude::*, rgb,
};
use nyaterm_store::{BootstrapSnapshot, LoadBootstrap, StoreDomain};
use nyaterm_transport::SftpDuplicatePolicy;
use nyaterm_ui::NyaDialogWindowExt;

use crate::features::{NyaTermApp, runtime_jobs::await_blocking_job, text_inputs::TextInputSetup};
use crate::models::{
    ConfigPathPromptKind, ConfigPathPromptResult, SnapshotPasswordPromptKind,
    TranslationSecretDraft,
};

fn snapshot_password_prompt_title_key(kind: SnapshotPasswordPromptKind) -> &'static str {
    match kind {
        SnapshotPasswordPromptKind::Export => "runtimePrompt.snapshotExport",
        SnapshotPasswordPromptKind::Import => "runtimePrompt.snapshotImport",
        SnapshotPasswordPromptKind::CloudForcePush => "runtimePrompt.cloudForcePush",
        SnapshotPasswordPromptKind::CloudForcePull => "runtimePrompt.cloudForcePull",
        SnapshotPasswordPromptKind::CloudProviderPush => "runtimePrompt.cloudProviderPush",
        SnapshotPasswordPromptKind::CloudProviderPull => "runtimePrompt.cloudProviderPull",
        SnapshotPasswordPromptKind::CloudProviderForcePush => {
            "runtimePrompt.cloudProviderForcePush"
        }
        SnapshotPasswordPromptKind::CloudProviderForcePull => {
            "runtimePrompt.cloudProviderForcePull"
        }
        SnapshotPasswordPromptKind::CloudRecoverCurrent
        | SnapshotPasswordPromptKind::CloudProviderRecoverCurrent => {
            "settings.useCurrentRemoteSnapshot"
        }
    }
}

impl NyaTermApp {
    pub(in crate::features) fn prompt_encrypted_portable_snapshot_export(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_snapshot_password_dialog(SnapshotPasswordPromptKind::Export, window, cx);
    }

    pub(in crate::features) fn prompt_encrypted_portable_snapshot_import(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.block_import_for_settings_draft(cx) {
            return;
        }
        if self.session.active_id().is_some() || self.session.start_has_pending() {
            self.shell
                .set_status(t!("settings.closeActiveSessionBeforeImport"));
            cx.notify();
            return;
        }
        self.open_confirm_dialog(
            (
                t!("savedConnections.restoreBackupConfirmTitle").to_string(),
                t!("savedConnections.restoreBackupConfirmDesc").to_string(),
                t!("savedConnections.restoreBackupConfirmAction").to_string(),
                true,
                |_app: &mut NyaTermApp, window: &mut Window, cx: &mut Context<NyaTermApp>| {
                    let app = cx.weak_entity();
                    let window = window.window_handle();
                    cx.defer(move |cx| {
                        let _ = window.update(cx, |_, window, cx| {
                            let _ = app.update(cx, |app, cx| {
                                app.open_snapshot_password_dialog(
                                    SnapshotPasswordPromptKind::Import,
                                    window,
                                    cx,
                                );
                            });
                        });
                    });
                    true
                },
            ),
            window,
            cx,
        );
    }

    fn open_snapshot_password_dialog(
        &mut self,
        kind: SnapshotPasswordPromptKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.clear_stale_local_snapshot_password_prompt(cx);
        if !self.settings.begin_snapshot_password_prompt(kind) {
            self.shell.set_status(t!("settings.backupOrSyncPromptOpen"));
            cx.notify();
            return;
        }

        self.forget_text_inputs("snapshot-password.");
        let field = self.text_input("snapshot-password.value", "", TextInputSetup::masked(), cx);
        self.shell.set_status(match kind {
            SnapshotPasswordPromptKind::Export => t!("settings.nyaExportPassword"),
            SnapshotPasswordPromptKind::Import => t!("settings.nyaImportPassword"),
            SnapshotPasswordPromptKind::CloudForcePush
            | SnapshotPasswordPromptKind::CloudForcePull
            | SnapshotPasswordPromptKind::CloudProviderPush
            | SnapshotPasswordPromptKind::CloudProviderPull
            | SnapshotPasswordPromptKind::CloudProviderForcePush
            | SnapshotPasswordPromptKind::CloudProviderForcePull
            | SnapshotPasswordPromptKind::CloudRecoverCurrent
            | SnapshotPasswordPromptKind::CloudProviderRecoverCurrent => {
                t!("settings.syncSnapshotPassword")
            }
        });
        self.settings
            .set_store_message(t!("settings.nyaAwaitingMasterPassword"));
        if !matches!(
            kind,
            SnapshotPasswordPromptKind::Export | SnapshotPasswordPromptKind::Import
        ) {
            self.cloud_sync
                .set_status(t!("settings.syncAwaitingPassword"));
        }
        let title = t!(snapshot_password_prompt_title_key(kind));
        self.open_form_dialog(
            (
                title.to_string(),
                448.,
                t!("runtimePrompt.submit").to_string(),
                |app, _, cx| app.snapshot_password_dialog_content(cx),
                |app, _, cx| app.submit_snapshot_password_prompt(cx),
                |app, cx| app.cancel_snapshot_password_prompt(cx),
            ),
            window,
            cx,
        );
        window.focus(&field.read(cx).focus_handle(), cx);
        cx.notify();
    }

    fn clear_stale_local_snapshot_password_prompt(&mut self, cx: &mut Context<Self>) {
        let Some(prompt) = self.settings.snapshot_password_prompt() else {
            return;
        };
        if matches!(
            prompt.kind,
            SnapshotPasswordPromptKind::Export | SnapshotPasswordPromptKind::Import
        ) {
            let _ = self.settings.take_snapshot_password_prompt();
            self.forget_text_inputs("snapshot-password.");
            self.settings.set_store_message("config picker cancelled");
            cx.notify();
        }
    }

    fn snapshot_password_dialog_content(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(prompt) = self.settings.snapshot_password_prompt() else {
            return div().into_any_element();
        };
        let palette = self.theme_palette();
        let description = if matches!(
            prompt.kind,
            SnapshotPasswordPromptKind::Export | SnapshotPasswordPromptKind::Import
        ) {
            t!("runtimePrompt.localSnapshotDescription")
        } else {
            t!("runtimePrompt.cloudSnapshotDescription")
        };
        let password_input = self.text_input_box(
            "snapshot-password.value",
            &prompt.value,
            TextInputSetup::masked(),
            cx,
        );

        div()
            .debug_selector(|| "snapshot-password-dialog-content".to_string())
            .flex()
            .flex_col()
            .gap_3()
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if this.handle_snapshot_password_key_down(event, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight(600.))
                    .text_color(rgb(palette.text))
                    .child(description),
            )
            .child(password_input)
            .into_any_element()
    }

    pub(in crate::features) fn start_snapshot_password_prompt(
        &mut self,
        kind: SnapshotPasswordPromptKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_snapshot_password_dialog(kind, window, cx);
    }

    fn submit_snapshot_password_prompt(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(state) = self.settings.take_snapshot_password_prompt() else {
            return true;
        };
        let password: nyaterm_core::SecretString = state.value.trim().to_owned().into();
        if password.is_empty() {
            self.settings.restore_snapshot_password_prompt(state.kind);
            self.reset_text_input("snapshot-password.value", "", cx);
            self.shell
                .set_status(t!("settings.nyaMasterPasswordRequired"));
            cx.notify();
            return false;
        }
        self.forget_text_inputs("snapshot-password.");

        match state.kind {
            SnapshotPasswordPromptKind::Export => {
                self.prompt_encrypted_portable_snapshot_export_path(password, cx);
            }
            SnapshotPasswordPromptKind::Import => {
                self.prompt_encrypted_portable_snapshot_import_path(password, cx);
            }
            SnapshotPasswordPromptKind::CloudForcePush => {
                self.run_local_cloud_sync_push(password, true, cx);
            }
            SnapshotPasswordPromptKind::CloudForcePull => {
                self.run_local_cloud_sync_pull(password, true, cx);
            }
            SnapshotPasswordPromptKind::CloudProviderPush => {
                self.run_provider_cloud_sync_push(password, false, cx);
            }
            SnapshotPasswordPromptKind::CloudProviderPull => {
                self.run_provider_cloud_sync_pull(password, false, cx);
            }
            SnapshotPasswordPromptKind::CloudProviderForcePush => {
                self.run_provider_cloud_sync_push(password, true, cx);
            }
            SnapshotPasswordPromptKind::CloudProviderForcePull => {
                self.run_provider_cloud_sync_pull(password, true, cx);
            }
            SnapshotPasswordPromptKind::CloudRecoverCurrent => {
                self.run_cloud_sync_recovery(password, false, cx);
            }
            SnapshotPasswordPromptKind::CloudProviderRecoverCurrent => {
                self.run_cloud_sync_recovery(password, true, cx);
            }
        }
        true
    }

    pub(in crate::features) fn cancel_snapshot_password_prompt(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.settings.take_snapshot_password_prompt() else {
            return;
        };
        self.forget_text_inputs("snapshot-password.");
        if !matches!(
            state.kind,
            SnapshotPasswordPromptKind::Export | SnapshotPasswordPromptKind::Import
        ) {
            self.cloud_sync.set_status(t!("settings.syncCancelled"));
        }
        self.shell.set_status(match state.kind {
            SnapshotPasswordPromptKind::Export => t!("settings.nyaExportCancelled"),
            SnapshotPasswordPromptKind::Import => t!("settings.nyaImportCancelled"),
            SnapshotPasswordPromptKind::CloudForcePush
            | SnapshotPasswordPromptKind::CloudProviderPush
            | SnapshotPasswordPromptKind::CloudProviderForcePush => {
                t!("settings.syncPushCancelled")
            }
            SnapshotPasswordPromptKind::CloudForcePull
            | SnapshotPasswordPromptKind::CloudProviderPull
            | SnapshotPasswordPromptKind::CloudProviderForcePull => {
                t!("settings.syncPullCancelled")
            }
            SnapshotPasswordPromptKind::CloudRecoverCurrent
            | SnapshotPasswordPromptKind::CloudProviderRecoverCurrent => {
                t!("settings.syncRecoveryCancelled")
            }
        });
        self.settings
            .set_store_message(t!("settings.configPathPickerClosed"));
        cx.notify();
    }

    pub(in crate::features) fn handle_snapshot_password_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.mark_user_activity();
        if !self.settings.snapshot_password_prompt_active() {
            return false;
        }
        let keystroke = &event.keystroke;
        if keystroke.modifiers.platform || keystroke.modifiers.alt || keystroke.modifiers.control {
            return false;
        }

        match keystroke.key.as_str() {
            "enter" => {
                if self.submit_snapshot_password_prompt(cx) {
                    window.close_nya_dialog(cx);
                }
            }
            "escape" => {
                self.cancel_snapshot_password_prompt(cx);
                window.close_nya_dialog(cx);
            }
            _ => return false,
        }
        true
    }

    pub(in crate::features) fn apply_snapshot_password_input(
        &mut self,
        text: String,
        cx: &mut Context<Self>,
    ) {
        if !self.settings.apply_snapshot_password_input(text) {
            return;
        }
        self.mark_user_activity();
        cx.notify();
    }

    fn prompt_encrypted_portable_snapshot_export_path(
        &mut self,
        master_password: nyaterm_core::SecretString,
        cx: &mut Context<Self>,
    ) {
        if !self
            .settings
            .begin_config_path_prompt(ConfigPathPromptKind::EncryptedPortableExport)
        {
            self.shell.set_status(t!("settings.configPathPickerOpen"));
            cx.notify();
            return;
        }
        let directory = self.runtime.config_dir().to_path_buf();
        let receiver = cx.prompt_for_new_path(&directory, Some("nyaterm-encrypted.nya"));
        let store = self.store_blocking_client();
        let scheduler = self.blocking_jobs.clone();
        self.shell
            .set_status(t!("settings.nyaSelectingDestination"));
        self.settings
            .set_store_message(t!("settings.nyaSelectingDestination"));
        self.request_settings_panel_refresh(cx);
        cx.spawn(async move |this, cx| {
            let result = match receiver.await {
                Ok(Ok(Some(path))) => {
                    let _ = this.update(cx, |this, cx| {
                        this.settings
                            .update_store_status(t!("settings.nyaExporting").to_string(), false);
                        this.request_settings_panel_refresh(cx);
                    });
                    tracing::info!(operation = "portable_snapshot_export", "started");
                    let task = scheduler.submit_task("portable-snapshot-export", move |_| {
                        match store.request_fn(StoreDomain::Settings, move |database| {
                            database.export_encrypted_portable_snapshot_from_open_store(
                                &path,
                                "native-local",
                                env!("CARGO_PKG_VERSION"),
                                master_password.expose_secret(),
                            )
                        }) {
                            Ok(info) => ConfigPathPromptResult::Exported(info),
                            Err(error) => ConfigPathPromptResult::Failed(error.to_string()),
                        }
                    });
                    await_blocking_job(task)
                        .await
                        .unwrap_or_else(ConfigPathPromptResult::Failed)
                }
                Ok(Ok(None)) => ConfigPathPromptResult::Cancelled,
                Ok(Err(error)) => ConfigPathPromptResult::Failed(error.to_string()),
                Err(_) => ConfigPathPromptResult::Closed,
            };
            let _ = this.update(cx, |this, cx| {
                this.apply_config_path_prompt_result(
                    ConfigPathPromptKind::EncryptedPortableExport,
                    result,
                    cx,
                );
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn prompt_encrypted_portable_snapshot_import_path(
        &mut self,
        master_password: nyaterm_core::SecretString,
        cx: &mut Context<Self>,
    ) {
        if !self
            .settings
            .begin_config_path_prompt(ConfigPathPromptKind::EncryptedPortableImport)
        {
            self.shell.set_status(t!("settings.configPathPickerOpen"));
            cx.notify();
            return;
        }
        let options = PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(SharedString::from("Select encrypted .nya snapshot")),
        };
        let receiver = cx.prompt_for_paths(options);
        let store = self.store_blocking_client();
        let scheduler = self.blocking_jobs.clone();
        self.shell.set_status(t!("settings.nyaSelectingSnapshot"));
        self.settings
            .set_store_message(t!("settings.nyaSelectingSnapshot"));
        self.request_settings_panel_refresh(cx);
        cx.spawn(async move |this, cx| {
            let result = match receiver.await {
                Ok(Ok(Some(paths))) => match paths.into_iter().next() {
                    Some(path) => {
                        let _ = this.update(cx, |this, cx| {
                            this.settings.update_store_status(
                                t!("settings.nyaImporting").to_string(),
                                false,
                            );
                            this.request_settings_panel_refresh(cx);
                        });
                        tracing::info!(operation = "portable_snapshot_import", "started");
                        let task =
                            scheduler.submit_task("portable-snapshot-import", move |_| match store
                                .request_fn(StoreDomain::Settings, move |database| {
                                    database.import_encrypted_portable_snapshot_into_open_store(
                                        &path,
                                        master_password.expose_secret(),
                                    )
                                }) {
                                Ok(info) => ConfigPathPromptResult::Imported(info),
                                Err(error) => ConfigPathPromptResult::Failed(error.to_string()),
                            });
                        await_blocking_job(task)
                            .await
                            .unwrap_or_else(ConfigPathPromptResult::Failed)
                    }
                    None => ConfigPathPromptResult::Cancelled,
                },
                Ok(Ok(None)) => ConfigPathPromptResult::Cancelled,
                Ok(Err(error)) => ConfigPathPromptResult::Failed(error.to_string()),
                Err(_) => ConfigPathPromptResult::Closed,
            };
            let _ = this.update(cx, |this, cx| {
                this.apply_config_path_prompt_result(
                    ConfigPathPromptKind::EncryptedPortableImport,
                    result,
                    cx,
                );
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn apply_config_path_prompt_result(
        &mut self,
        kind: ConfigPathPromptKind,
        result: ConfigPathPromptResult,
        cx: &mut Context<Self>,
    ) {
        if !self.settings.finish_config_path_prompt(kind) {
            return;
        }
        match result {
            ConfigPathPromptResult::Exported(info) => {
                tracing::info!(
                    operation = "portable_snapshot_export",
                    bytes = info.bytes,
                    "completed"
                );
                let message = t!("settings.nyaExportedCount", count = info.bytes).to_string();
                self.settings.replace_store_status(
                    info.database_path.display().to_string(),
                    message,
                    true,
                );
                self.shell.set_status(t!(
                    "settings.nyaExportedTo",
                    path = info.backup_path.display().to_string()
                ));
            }
            ConfigPathPromptResult::Imported(info) => {
                tracing::info!(
                    operation = "portable_snapshot_import",
                    bytes = info.bytes,
                    safety_backup = info.safety_backup_path.is_some(),
                    "completed"
                );
                let safety = info
                    .safety_backup_path
                    .as_ref()
                    .map(|path| {
                        format!(
                            "; {}",
                            t!(
                                "settings.nyaPreviousDbSavedTo",
                                path = path.display().to_string()
                            )
                        )
                    })
                    .unwrap_or_default();
                let message = format!(
                    "{}{}",
                    t!("settings.nyaImportedCount", count = info.bytes),
                    safety
                );
                self.refresh_store_after_portable_import(message, cx);
                self.shell.set_status(t!(
                    "settings.nyaImportedFrom",
                    path = info.backup_path.display().to_string()
                ));
            }
            ConfigPathPromptResult::Cancelled => {
                tracing::info!(operation = ?kind, "portable snapshot picker cancelled");
                self.shell.set_status(match kind {
                    ConfigPathPromptKind::EncryptedPortableExport => {
                        t!("settings.nyaExportCancelled")
                    }
                    ConfigPathPromptKind::EncryptedPortableImport => {
                        t!("settings.nyaImportCancelled")
                    }
                });
                self.settings
                    .set_store_message(t!("settings.configPathPickerClosed"));
            }
            ConfigPathPromptResult::Failed(error) => {
                tracing::warn!(operation = ?kind, error = %error, "portable snapshot operation failed");
                self.shell.set_status(match kind {
                    ConfigPathPromptKind::EncryptedPortableExport => {
                        format!("{}: {error}", t!("settings.nyaExportFailed"))
                    }
                    ConfigPathPromptKind::EncryptedPortableImport => {
                        format!("{}: {error}", t!("settings.nyaImportFailed"))
                    }
                });
                self.settings
                    .update_store_status(self.shell.status().to_string(), false);
            }
            ConfigPathPromptResult::Closed => {
                tracing::warn!(operation = ?kind, "portable snapshot picker closed");
                self.shell.set_status(t!("settings.configPathPickerClosed"));
                self.settings
                    .set_store_message(t!("settings.configPathPickerClosed"));
            }
        }
        self.request_settings_panel_refresh(cx);
    }

    fn refresh_store_after_portable_import(
        &mut self,
        success_message: String,
        cx: &mut Context<Self>,
    ) {
        self.submit_store_request(
            0,
            LoadBootstrap,
            move |this, event, cx| match event.outcome {
                Ok(snapshot) => {
                    this.apply_store_refresh(snapshot.clone(), cx);
                    this.replace_shared_snapshot(
                        snapshot,
                        crate::app_shell::SharedStateDomain::All,
                        cx,
                    );
                    this.rebase_open_settings_draft(cx);
                    this.settings.update_store_status(success_message, true);
                    this.request_settings_panel_refresh(cx);
                    cx.notify();
                }
                Err(error) => {
                    let message = format!("store refresh after import failed: {error}");
                    tracing::warn!(error = %error, "portable snapshot import refresh failed");
                    this.settings.update_store_status(message.clone(), false);
                    this.shell.set_status(message);
                    this.request_settings_panel_refresh(cx);
                    cx.notify();
                }
            },
            cx,
        );
    }

    pub(in crate::features) fn refresh_store_from_runtime_and_sync_theme(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        self.submit_store_request(
            0,
            LoadBootstrap,
            |this, event, cx| match event.outcome {
                Ok(snapshot) => {
                    this.apply_store_refresh(snapshot.clone(), cx);
                    this.replace_shared_snapshot(
                        snapshot,
                        crate::app_shell::SharedStateDomain::All,
                        cx,
                    );
                    cx.notify();
                }
                Err(error) => {
                    let message = format!("store refresh failed: {error}");
                    this.settings.update_store_status(message.clone(), false);
                    this.shell.set_status(message);
                    cx.notify();
                }
            },
            cx,
        );
    }

    fn apply_store_refresh(&mut self, snapshot: BootstrapSnapshot, cx: &mut Context<Self>) {
        let mut shared_settings = snapshot.settings;
        let local_settings = self.settings.summary();
        shared_settings.ui_left_panel_width = local_settings.ui_left_panel_width;
        shared_settings.ui_right_panel_width = local_settings.ui_right_panel_width;
        shared_settings.ui_quick_cmd_height = local_settings.ui_quick_cmd_height;
        shared_settings.ui_active_left_panel = local_settings.ui_active_left_panel.clone();
        shared_settings.ui_active_right_panel = local_settings.ui_active_right_panel.clone();
        shared_settings.ui_left_panel_collapsed = local_settings.ui_left_panel_collapsed;
        shared_settings.ui_right_panel_collapsed = local_settings.ui_right_panel_collapsed;
        self.update_custom_icons(snapshot.custom_icons, cx);
        self.connection_state
            .replace_loaded(snapshot.connections, snapshot.connection_groups);
        self.security.replace_catalog(
            snapshot.ssh_keys,
            snapshot.otp_entries,
            snapshot.saved_passwords,
            snapshot.saved_credentials,
        );
        self.tunnel_state.replace_loaded_catalog(
            snapshot.tunnels,
            snapshot.tunnel_groups,
            snapshot.proxies,
            snapshot.proxy_groups,
        );
        self.commands.replace_loaded(
            snapshot.quick_commands,
            snapshot.quick_command_categories,
            snapshot.command_history,
        );
        self.settings
            .replace_keyword_config(snapshot.keyword_highlights);
        self.apply_gpui_settings(shared_settings, cx);
        self.translation.replace_settings(
            snapshot.translation_settings,
            TranslationSecretDraft::default(),
        );
        self.recording
            .set_memory_limit(self.settings.summary().recording_memory_limit_bytes as usize);
        self.ai.replace_settings_config(snapshot.ai_settings, true);
        self.sync_ai_drafts_from_active_profile();
        self.settings.rebase_master_password();
        self.cloud_sync
            .replace_loaded(snapshot.cloud_sync_settings, snapshot.cloud_sync_state);
        self.transfer
            .set_duplicate_policy(SftpDuplicatePolicy::from_legacy_value(
                &self.settings.summary().transfer_duplicate_strategy,
            ));
        self.settings.replace_store_status(
            snapshot.database_path.display().to_string(),
            t!("settings.redbStoreOnline").to_string(),
            true,
        );
        // Notes are not part of BootstrapSnapshot because their Markdown bodies
        // are loaded on demand. A backup import or cloud pull may have replaced
        // the entire catalog, so refresh its lightweight tree explicitly.
        self.refresh_notes(cx);
        self.request_settings_panel_refresh(cx);
    }

    pub(crate) fn apply_shared_state(
        &mut self,
        snapshot: BootstrapSnapshot,
        event: crate::app_shell::SharedStateEvent,
        cx: &mut Context<Self>,
    ) {
        use crate::app_shell::SharedStateDomain;

        let has_clean_settings_draft =
            self.shell.has_settings_draft() && !self.settings_draft_dirty();
        let settings_blocked = self.settings_draft_dirty()
            && matches!(
                event.domain,
                SharedStateDomain::Settings
                    | SharedStateDomain::Ai
                    | SharedStateDomain::Translation
                    | SharedStateDomain::CloudSync
                    | SharedStateDomain::All
            );
        if settings_blocked {
            self.shell.set_status(format!(
                "shared settings changed at revision {}; reload before applying",
                event.revision
            ));
        }

        if matches!(
            event.domain,
            SharedStateDomain::Connections | SharedStateDomain::All
        ) {
            self.update_custom_icons(snapshot.custom_icons.clone(), cx);
            self.connection_state.replace_loaded(
                snapshot.connections.clone(),
                snapshot.connection_groups.clone(),
            );
            self.start_workspace
                .sync_group_options(&snapshot.connection_groups, cx);
        }
        if matches!(
            event.domain,
            SharedStateDomain::Security | SharedStateDomain::All
        ) {
            self.security.replace_catalog(
                snapshot.ssh_keys.clone(),
                snapshot.otp_entries.clone(),
                snapshot.saved_passwords.clone(),
                snapshot.saved_credentials.clone(),
            );
        }
        if matches!(
            event.domain,
            SharedStateDomain::Tunnels | SharedStateDomain::All
        ) {
            self.tunnel_state.replace_loaded_catalog(
                snapshot.tunnels.clone(),
                snapshot.tunnel_groups.clone(),
                snapshot.proxies.clone(),
                snapshot.proxy_groups.clone(),
            );
        }
        if matches!(
            event.domain,
            SharedStateDomain::Commands | SharedStateDomain::All
        ) {
            self.commands.replace_loaded(
                snapshot.quick_commands.clone(),
                snapshot.quick_command_categories.clone(),
                snapshot.command_history.clone(),
            );
        }
        if !settings_blocked
            && matches!(
                event.domain,
                SharedStateDomain::Settings | SharedStateDomain::All
            )
        {
            let mut settings = snapshot.settings.clone();
            let local = self.settings.summary();
            settings.ui_left_panel_width = local.ui_left_panel_width;
            settings.ui_right_panel_width = local.ui_right_panel_width;
            settings.ui_quick_cmd_height = local.ui_quick_cmd_height;
            settings.ui_active_left_panel = local.ui_active_left_panel.clone();
            settings.ui_active_right_panel = local.ui_active_right_panel.clone();
            settings.ui_left_panel_collapsed = local.ui_left_panel_collapsed;
            settings.ui_right_panel_collapsed = local.ui_right_panel_collapsed;
            self.settings
                .replace_keyword_config(snapshot.keyword_highlights.clone());
            self.apply_gpui_settings(settings, cx);
            self.recording
                .set_memory_limit(self.settings.summary().recording_memory_limit_bytes as usize);
            self.transfer
                .set_duplicate_policy(SftpDuplicatePolicy::from_legacy_value(
                    &self.settings.summary().transfer_duplicate_strategy,
                ));
        }
        if matches!(
            event.domain,
            SharedStateDomain::Commands | SharedStateDomain::All
        ) {
            self.sync_quick_command_selected_category(cx);
        }
        if !settings_blocked
            && matches!(event.domain, SharedStateDomain::Ai | SharedStateDomain::All)
        {
            self.ai.replace_settings_config(snapshot.ai_settings, true);
            self.sync_ai_drafts_from_active_profile();
        }
        if !settings_blocked
            && matches!(
                event.domain,
                SharedStateDomain::Translation | SharedStateDomain::All
            )
        {
            self.translation.replace_settings(
                snapshot.translation_settings,
                TranslationSecretDraft::default(),
            );
        }
        if !settings_blocked
            && matches!(
                event.domain,
                SharedStateDomain::CloudSync | SharedStateDomain::All
            )
        {
            self.cloud_sync
                .replace_loaded(snapshot.cloud_sync_settings, snapshot.cloud_sync_state);
        }
        if has_clean_settings_draft
            && matches!(
                event.domain,
                SharedStateDomain::Settings
                    | SharedStateDomain::Ai
                    | SharedStateDomain::Translation
                    | SharedStateDomain::CloudSync
                    | SharedStateDomain::All
            )
        {
            self.rebase_open_settings_draft(cx);
        }
        self.flush_connection_panel_snapshot(cx);
        self.request_settings_panel_refresh(cx);
        cx.notify();
    }
}
