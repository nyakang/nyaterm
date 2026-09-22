use rust_i18n::t;

use gpui::{
    AnyElement, App, ClickEvent, Context, FontWeight, IntoElement, SharedString, Window, div,
    prelude::*, px, rgb, rgba,
};
use nyaterm_core::{CloudConflictKind, CloudSyncSettings};
use nyaterm_ui::NyaSelectOption;

use crate::features::sync::CloudSyncLiveState;
use crate::features::{
    formatting::compact_id, formatting::configured_cloud_sync_provider,
    formatting::format_cloud_provider, formatting::format_history_timestamp_ms,
    pages::settings::panel::SettingsPanel, view_widgets::dialog_action_button,
};
use crate::models::{CloudSyncConflictState, CloudSyncInputField, SettingsTab};
use crate::theme::ThemePalette;
use crate::widgets::small_button;

use super::super::{settings_form_row, settings_form_section, settings_switch_with_enabled};

mod providers;
impl SettingsPanel {
    fn cloud_sync_provider_select(
        &mut self,
        active_provider: &str,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let options = [
            ("webdav", "WebDAV"),
            ("s3", "S3 Compatible"),
            ("gitee_snippet", "Gitee Snippet"),
            ("github_gist", "GitHub Gist"),
            ("google_drive", "Google Drive"),
            ("onedrive", "OneDrive"),
            ("aliyun_drive", "AliyunDrive"),
        ]
        .into_iter()
        .map(|(provider, label)| NyaSelectOption::new(provider, label))
        .collect();
        self.select_control(
            "cloud-provider-select",
            options,
            Some(active_provider.to_string()),
            !enabled,
            cx,
        )
        .into_any_element()
    }

    pub(in crate::features) fn cloud_sync_input(
        &mut self,
        _id: &'static str,
        label: impl Into<SharedString>,
        value: String,
        field: CloudSyncInputField,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let label: SharedString = label.into();
        let enabled = self.cloud_sync_form_enabled();
        let _ = (value, cx);
        let input = self.existing_text_input_field(
            format!("cloud-sync.input.{}", field.input_key()),
            label,
            false,
        );
        // A settings row's control slot is content-sized, and a box with nothing
        // typed in it has no content — so the width comes from here.
        div()
            .w(px(260.))
            .flex()
            .opacity(if enabled { 1.0 } else { 0.45 })
            .child(div().min_w_0().flex_1().child(input))
            .into_any_element()
    }

    pub(in crate::features) fn cloud_sync_conflict_banner(
        &mut self,
        conflict: CloudSyncConflictState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let provider_action = conflict.provider_action;
        let preview = conflict.preview;
        let remote_inconsistent = preview.kind == CloudConflictKind::RemoteInconsistent;
        let local_hash = compact_id(&preview.local_payload_hash);
        let remote_revision = format!(
            "{} / {}",
            compact_id(&preview.remote_revision),
            format_history_timestamp_ms(preview.remote_created_at_ms)
        );
        let recovery_candidate = preview.recovery_revision.as_deref().map(|revision| {
            let hash = preview
                .recovery_payload_hash
                .as_deref()
                .map(compact_id)
                .unwrap_or_else(|| "unknown".to_string());
            let timestamp = preview
                .recovery_created_at_ms
                .map(format_history_timestamp_ms)
                .unwrap_or_else(|| "unknown".to_string());
            format!("{} / {hash} / {timestamp}", compact_id(revision))
        });

        div()
            .rounded_md()
            .border_1()
            .border_color(rgba((palette.warning << 8) | 0x4d))
            .bg(rgba((palette.warning << 8) | 0x1a))
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight(700.))
                            .text_color(rgb(palette.text))
                            .child(if remote_inconsistent {
                                t!("settings.syncRemoteIncompleteTitle")
                            } else {
                                t!("settings.syncConflictTitle")
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .line_height(px(18.))
                            .text_color(rgb(palette.text_muted))
                            .child(preview.message.clone()),
                    ),
            )
            .child(
                div()
                    .grid()
                    .grid_cols(2)
                    .gap_2()
                    .child(cloud_sync_conflict_stat(
                        palette,
                        t!("settings.localSnapshot"),
                        local_hash,
                    ))
                    .child(cloud_sync_conflict_stat(
                        palette,
                        t!("settings.remoteSnapshot"),
                        remote_revision,
                    ))
                    .when_some(recovery_candidate, |this, recovery_candidate| {
                        this.child(cloud_sync_conflict_stat(
                            palette,
                            t!("settings.currentRemoteSnapshot"),
                            recovery_candidate,
                        ))
                    }),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(palette.text_muted))
                    .child(format_cloud_provider(&preview.provider)),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(if remote_inconsistent {
                        small_button(
                            palette,
                            "cloud-conflict-recover-current",
                            t!("settings.useCurrentRemoteSnapshot"),
                            cx.listener(move |this, _, window, cx| {
                                this.prompt_cloud_sync_recover_current(provider_action, window, cx);
                            }),
                        )
                        .into_any_element()
                    } else {
                        small_button(
                            palette,
                            "cloud-conflict-force-pull",
                            t!("settings.downloadRemoteVersion"),
                            cx.listener(move |this, _, window, cx| {
                                this.prompt_cloud_sync_force_pull(provider_action, window, cx);
                            }),
                        )
                        .into_any_element()
                    })
                    .child(dialog_action_button(
                        palette,
                        "cloud-conflict-force-push",
                        t!("settings.uploadLocalVersion"),
                        false,
                        cx.listener(move |this, _, window, cx| {
                            this.prompt_cloud_sync_force_push(provider_action, window, cx);
                        }),
                    )),
            )
    }

    pub(in crate::features) fn cloud_sync_settings_section(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let cloud_conflict = self.cloud_sync.conflict().cloned();
        let active_cloud_provider = configured_cloud_sync_provider(self.cloud_sync.settings());
        let form_enabled = self.cloud_sync_form_enabled();
        let auto_sync_enabled = form_enabled && self.cloud_sync.settings().enabled;
        let _debounce_enabled = auto_sync_enabled && self.cloud_sync.settings().auto_push_on_change;
        let validation_key = cloud_sync_validation_key(&self.cloud_sync.pending_settings());
        let validation_message = (form_enabled && self.cloud_sync.settings().enabled)
            .then(|| validation_key.map(|key| t!(key)))
            .flatten();
        let settings_dirty = self.settings_draft_dirty();
        let action_block_message = if !form_enabled {
            Some(t!("settings.masterPasswordRequiredDesc"))
        } else if settings_dirty {
            Some(t!("settings.applySettingsFirst"))
        } else {
            validation_key.map(|key| t!(key))
        };
        let prompt_busy = self.settings.snapshot_password_prompt_active()
            || self.settings.config_path_prompt_active();
        let local_backup_status = self.settings.local_backup_status.clone();
        let local_backup_ready = self.settings.local_backup_ready;
        let actions_busy = prompt_busy || self.cloud_sync.job_running();
        let can_run_actions = action_block_message.is_none() && !actions_busy;
        let can_run_enabled_actions = can_run_actions && self.cloud_sync.settings().enabled;
        let sync_state_key = cloud_sync_state_i18n_key(
            self.cloud_sync.settings().enabled,
            cloud_conflict.is_some(),
            self.cloud_sync.live_state(),
            self.cloud_sync
                .history()
                .first()
                .map(|entry| entry.status.as_str()),
        );
        let sync_running = sync_state_key == "settings.syncState.running";
        let current_operation = if sync_running {
            self.cloud_sync.status().to_string()
        } else {
            t!("settings.none").to_string()
        };
        let provider_label = cloud_sync_provider_label(&active_cloud_provider).to_string();
        let last_checked = self
            .cloud_sync
            .state()
            .last_checked_at_ms
            .map(format_history_timestamp_ms)
            .unwrap_or_else(|| t!("settings.never").to_string());
        let last_synced = self
            .cloud_sync
            .state()
            .last_synced_at_ms
            .map(format_history_timestamp_ms)
            .unwrap_or_else(|| t!("settings.never").to_string());
        let webdav_password_value = self
            .cloud_sync
            .secret_draft()
            .webdav_password
            .expose_secret()
            .to_owned();
        let s3_access_key_value = self
            .cloud_sync
            .secret_draft()
            .s3_access_key_id
            .expose_secret()
            .to_owned();
        let s3_secret_key_value = self
            .cloud_sync
            .secret_draft()
            .s3_secret_access_key
            .expose_secret()
            .to_owned();
        let s3_session_token_value = self
            .cloud_sync
            .secret_draft()
            .s3_session_token
            .expose_secret()
            .to_owned();
        let google_drive_access_token_value = self
            .cloud_sync
            .secret_draft()
            .google_drive_access_token
            .expose_secret()
            .to_owned();
        let google_drive_refresh_token_value = self
            .cloud_sync
            .secret_draft()
            .google_drive_refresh_token
            .expose_secret()
            .to_owned();
        let google_drive_client_secret_value = self
            .cloud_sync
            .secret_draft()
            .google_drive_client_secret
            .expose_secret()
            .to_owned();
        let onedrive_access_token_value = self
            .cloud_sync
            .secret_draft()
            .onedrive_access_token
            .expose_secret()
            .to_owned();
        let onedrive_refresh_token_value = self
            .cloud_sync
            .secret_draft()
            .onedrive_refresh_token
            .expose_secret()
            .to_owned();
        let onedrive_client_secret_value = self
            .cloud_sync
            .secret_draft()
            .onedrive_client_secret
            .expose_secret()
            .to_owned();
        let aliyun_drive_access_token_value = self
            .cloud_sync
            .secret_draft()
            .aliyun_drive_access_token
            .expose_secret()
            .to_owned();
        let aliyun_drive_refresh_token_value = self
            .cloud_sync
            .secret_draft()
            .aliyun_drive_refresh_token
            .expose_secret()
            .to_owned();
        let aliyun_drive_client_secret_value = self
            .cloud_sync
            .secret_draft()
            .aliyun_drive_client_secret
            .expose_secret()
            .to_owned();
        let gitee_token_value = self
            .cloud_sync
            .secret_draft()
            .gitee_token
            .expose_secret()
            .to_owned();
        let provider_fields = match active_cloud_provider.as_str() {
            "webdav" => self.cloud_sync_webdav_provider_fields(webdav_password_value, cx),
            "s3" => self.cloud_sync_s3_provider_fields(
                s3_access_key_value,
                s3_secret_key_value,
                s3_session_token_value,
                cx,
            ),
            "google_drive" => self.cloud_sync_oauth_provider_fields(
                "google_drive",
                google_drive_access_token_value,
                google_drive_refresh_token_value,
                google_drive_client_secret_value,
                cx,
            ),
            "onedrive" => self.cloud_sync_oauth_provider_fields(
                "onedrive",
                onedrive_access_token_value,
                onedrive_refresh_token_value,
                onedrive_client_secret_value,
                cx,
            ),
            "aliyun_drive" => self.cloud_sync_aliyun_provider_fields(
                aliyun_drive_access_token_value,
                aliyun_drive_refresh_token_value,
                aliyun_drive_client_secret_value,
                cx,
            ),
            "gitee_snippet" => self.cloud_sync_gitee_provider_fields(gitee_token_value, cx),
            "github_gist" => self.cloud_sync_github_provider_fields(cx),
            _ => div().into_any_element(),
        };

        div()
            .flex()
            .flex_col()
            .gap_5()
            .child(settings_form_section(
                palette,
                Some(t!("settings.localBackup")),
                Some(t!("settings.localBackupDesc")),
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(settings_form_row(
                        palette,
                        t!("settings.exportConfig"),
                        Some(SharedString::from(t!("settings.exportConfigDesc"))),
                        cloud_sync_action_button(
                            palette,
                            "settings-local-backup-export",
                            t!("settings.exportConfig"),
                            !actions_busy,
                            cx.listener(|this, _, window, cx| {
                                this.prompt_encrypted_portable_snapshot_export(window, cx);
                            }),
                        ),
                    ))
                    .child(settings_form_row(
                        palette,
                        t!("settings.importConfig"),
                        Some(SharedString::from(t!("settings.importConfigDesc"))),
                        cloud_sync_action_button(
                            palette,
                            "settings-local-backup-import",
                            t!("settings.importConfig"),
                            !actions_busy,
                            cx.listener(|this, _, window, cx| {
                                this.prompt_encrypted_portable_snapshot_import(window, cx);
                            }),
                        ),
                    ))
                    .when(!local_backup_status.is_empty(), |this| {
                        this.child(
                            div()
                                .min_w_0()
                                .rounded_md()
                                .border_1()
                                .border_color(rgb(if local_backup_ready {
                                    palette.success
                                } else {
                                    palette.border
                                }))
                                .bg(rgb(palette.surface_elevated))
                                .px_3()
                                .py_2()
                                .text_size(px(12.))
                                .text_color(rgb(palette.text_muted))
                                .child(local_backup_status.clone()),
                        )
                    }),
            ))
            .child(settings_form_section(
                palette,
                Some(t!("settings.syncProviderConfig")),
                Some(t!("settings.syncProviderConfigDesc")),
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(settings_form_row(
                        palette,
                        t!("settings.enableCloudSync"),
                        Some(SharedString::from(t!("settings.enableCloudSyncDesc"))),
                        settings_switch_with_enabled(
                            palette,
                            "cloud-sync-enabled",
                            self.cloud_sync.settings().enabled,
                            form_enabled,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_cloud_sync_enabled(cx);
                            }),
                        ),
                    ))
                    .when(!form_enabled, |this| {
                        this.child(
                            div()
                                .rounded_md()
                                .border_1()
                                .border_color(rgb(palette.warning))
                                .bg(rgba((palette.warning << 8) | 0x14))
                                .p_3()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_3()
                                .child(
                                    div()
                                        .min_w_0()
                                        .text_size(px(12.))
                                        .text_color(rgb(palette.text_muted))
                                        .child(t!("settings.syncMasterPasswordMissingDesc")),
                                )
                                .child(small_button(
                                    palette,
                                    "cloud-open-security",
                                    t!("settings.openSecuritySettings"),
                                    cx.listener(|this, _, _, cx| {
                                        this.with_app(cx, |app, cx| {
                                            app.focus_settings_tab(SettingsTab::Security, cx);
                                        });
                                    }),
                                )),
                        )
                    })
                    .when_some(validation_message, |this, message| {
                        this.child(
                            div()
                                .rounded_md()
                                .border_1()
                                .border_color(rgb(palette.warning))
                                .bg(rgba((palette.warning << 8) | 0x14))
                                .px_3()
                                .py_2()
                                .text_size(px(12.))
                                .text_color(rgb(palette.text_muted))
                                .child(message),
                        )
                    })
                    .child(settings_form_row(
                        palette,
                        t!("settings.syncProvider"),
                        Some(SharedString::from(t!("settings.syncProviderDesc"))),
                        self.cloud_sync_provider_select(&active_cloud_provider, form_enabled, cx),
                    ))
                    .child(settings_form_row(
                        palette,
                        t!("settings.deviceName"),
                        Some(SharedString::from(t!("settings.deviceNameDesc"))),
                        self.cloud_sync_input(
                            "cloud-sync-device-name",
                            t!("settings.deviceName"),
                            self.cloud_sync.settings().device_name.clone(),
                            CloudSyncInputField::DeviceName,
                            cx,
                        ),
                    ))
                    .child(settings_form_row(
                        palette,
                        t!("settings.remoteNamespace"),
                        Some(SharedString::from(t!("settings.remoteNamespaceDesc"))),
                        self.cloud_sync_input(
                            "cloud-sync-remote-root",
                            t!("settings.remoteNamespace"),
                            self.cloud_sync.settings().remote_root.clone(),
                            CloudSyncInputField::RemoteRoot,
                            cx,
                        ),
                    ))
                    .child(provider_fields),
            ))
            .child(settings_form_section(
                palette,
                Some(t!("settings.autoSyncStrategy")),
                Some(t!("settings.autoSyncStrategyDesc")),
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .when_some(action_block_message, |this, message| {
                        this.child(
                            div()
                                .rounded_md()
                                .border_1()
                                .border_color(rgb(palette.warning))
                                .bg(rgba((palette.warning << 8) | 0x14))
                                .px_3()
                                .py_2()
                                .text_size(px(12.))
                                .text_color(rgb(palette.text_muted))
                                .child(message),
                        )
                    })
                    .child(settings_form_row(
                        palette,
                        t!("settings.autoCheckOnStartup"),
                        Some(SharedString::from(t!("settings.autoCheckOnStartupDesc"))),
                        settings_switch_with_enabled(
                            palette,
                            "cloud-auto-check",
                            self.cloud_sync.settings().auto_check_on_startup,
                            auto_sync_enabled,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_cloud_sync_auto_check(cx);
                            }),
                        ),
                    ))
                    .child(settings_form_row(
                        palette,
                        t!("settings.autoPushOnChange"),
                        Some(SharedString::from(t!("settings.autoPushOnChangeDesc"))),
                        settings_switch_with_enabled(
                            palette,
                            "cloud-auto-push",
                            self.cloud_sync.settings().auto_push_on_change,
                            auto_sync_enabled,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_cloud_sync_auto_push(cx);
                            }),
                        ),
                    ))
                    .child(settings_form_row(
                        palette,
                        t!("settings.autoPullRemoteChanges"),
                        Some(SharedString::from(t!("settings.autoPullRemoteChangesDesc"))),
                        settings_switch_with_enabled(
                            palette,
                            "cloud-auto-pull-remote-changes",
                            self.cloud_sync.settings().auto_pull_remote_changes,
                            auto_sync_enabled,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_cloud_sync_auto_pull_remote_changes(cx);
                            }),
                        ),
                    ))
                    .child(settings_form_row(
                        palette,
                        t!("settings.syncDebounceSeconds"),
                        Some(SharedString::from(t!("settings.syncDebounceSecondsDesc"))),
                        self.existing_number_input_box("cloud-sync.number.debounce"),
                    )),
            ))
            .child(settings_form_section(
                palette,
                Some(t!("settings.manualSyncActions")),
                None,
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .grid()
                            .grid_cols(2)
                            .gap_2()
                            .child(cloud_sync_status_item(
                                palette,
                                t!("settings.syncStatus"),
                                t!(sync_state_key).to_string(),
                            ))
                            .child(cloud_sync_status_item(
                                palette,
                                t!("settings.syncProvider"),
                                provider_label,
                            ))
                            .child(cloud_sync_status_item(
                                palette,
                                t!("settings.lastSyncCheck"),
                                last_checked,
                            ))
                            .child(cloud_sync_status_item(
                                palette,
                                t!("settings.lastSyncAt"),
                                last_synced,
                            ))
                            .child(cloud_sync_status_item(
                                palette,
                                t!("settings.currentOperation"),
                                current_operation,
                            )),
                    )
                    .when(!self.cloud_sync.status().is_empty(), |this| {
                        this.child(
                            div()
                                .min_w_0()
                                .rounded_md()
                                .border_1()
                                .border_color(rgb(palette.border))
                                .bg(rgb(palette.surface_elevated))
                                .px_3()
                                .py_2()
                                .text_size(px(12.))
                                .text_color(rgb(palette.text_muted))
                                .child(self.cloud_sync.status().to_string()),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_1()
                            .child(cloud_sync_action_button(
                                palette,
                                "settings-provider-cloud-sync-test",
                                t!("settings.testConnection"),
                                can_run_actions,
                                cx.listener(|this, _, _, cx| {
                                    this.run_provider_cloud_sync_test(cx);
                                }),
                            ))
                            .child(cloud_sync_action_button(
                                palette,
                                "settings-provider-cloud-sync-push",
                                t!("settings.syncPushNow"),
                                can_run_enabled_actions,
                                cx.listener(|this, _, window, cx| {
                                    this.prompt_provider_cloud_sync_push(window, cx);
                                }),
                            ))
                            .child(cloud_sync_action_button(
                                palette,
                                "settings-provider-cloud-sync-pull",
                                t!("settings.syncPullNow"),
                                can_run_enabled_actions,
                                cx.listener(|this, _, window, cx| {
                                    this.prompt_provider_cloud_sync_pull(window, cx);
                                }),
                            )),
                    ),
            ))
            .child(settings_form_section(
                palette,
                Some(t!("settings.syncConflictSection")),
                Some(t!("settings.syncConflictSectionDesc")),
                if let Some(conflict) = cloud_conflict {
                    self.cloud_sync_conflict_banner(conflict, cx)
                        .into_any_element()
                } else {
                    div()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(palette.border))
                        .px_4()
                        .py_5()
                        .text_center()
                        .text_size(px(12.))
                        .text_color(rgb(palette.text_muted))
                        .child(t!("settings.noSyncConflict"))
                        .into_any_element()
                },
            ))
    }
}

fn cloud_sync_conflict_stat(
    palette: crate::theme::ThemePalette,
    label: impl Into<SharedString>,
    value: String,
) -> impl IntoElement {
    let label: SharedString = label.into();
    div()
        .min_w_0()
        .rounded_md()
        .border_1()
        .border_color(rgb(palette.border))
        .bg(rgb(palette.input))
        .p_3()
        .child(
            div()
                .text_size(px(11.))
                .text_color(rgb(palette.text_muted))
                .child(label),
        )
        .child(
            div()
                .mt_2()
                .font_family(crate::features::shell::gpui_code_font_family())
                .text_xs()
                .text_color(rgb(palette.text))
                .child(value),
        )
}

fn cloud_sync_provider_label(provider: &str) -> &'static str {
    match provider {
        "webdav" => "WebDAV",
        "s3" => "S3 Compatible",
        "gitee_snippet" => "Gitee Snippet",
        "github_gist" => "GitHub Gist",
        "google_drive" => "Google Drive",
        "onedrive" => "OneDrive",
        "aliyun_drive" => "AliyunDrive",
        _ => "-",
    }
}

fn cloud_sync_state_i18n_key(
    enabled: bool,
    has_conflict: bool,
    live_state: CloudSyncLiveState,
    last_history_status: Option<&str>,
) -> &'static str {
    if has_conflict {
        return "settings.syncState.conflict";
    }
    if !enabled {
        return "settings.syncState.disabled";
    }
    match live_state {
        CloudSyncLiveState::Running => return "settings.syncState.running",
        CloudSyncLiveState::Failed => return "settings.syncState.failed",
        CloudSyncLiveState::Success => return "settings.syncState.success",
        CloudSyncLiveState::Idle => {}
    }
    match last_history_status {
        Some("failed") => "settings.syncState.failed",
        Some("success") => "settings.syncState.success",
        _ => "settings.syncState.idle",
    }
}

fn cloud_sync_validation_key(settings: &CloudSyncSettings) -> Option<&'static str> {
    match settings.provider.as_str() {
        "webdav" if settings.webdav.endpoint.trim().is_empty() => {
            Some("settings.webdavEndpointRequired")
        }
        "s3" if settings.s3.endpoint.trim().is_empty() => Some("settings.s3EndpointRequired"),
        "s3" if settings.s3.bucket.trim().is_empty() => Some("settings.s3BucketRequired"),
        "s3" if settings
            .s3
            .access_key_id
            .as_deref()
            .unwrap_or_default()
            .trim()
            .is_empty()
            != settings
                .s3
                .secret_access_key
                .as_deref()
                .unwrap_or_default()
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
                .unwrap_or_default()
                .trim()
                .is_empty() =>
        {
            Some("settings.giteeSnippetTokenRequired")
        }
        "google_drive" => cloud_sync_drive_validation_key(
            settings.google_drive.refresh_token.as_deref(),
            settings.google_drive.client_id.as_deref(),
            settings.google_drive.client_secret.as_deref(),
        ),
        "onedrive" => cloud_sync_drive_validation_key(
            settings.onedrive.refresh_token.as_deref(),
            settings.onedrive.client_id.as_deref(),
            settings.onedrive.client_secret.as_deref(),
        ),
        "aliyun_drive" => cloud_sync_drive_validation_key(
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
                .unwrap_or_default()
                .trim()
                .is_empty() =>
        {
            Some("settings.githubGistTokenRequired")
        }
        _ => None,
    }
}

fn cloud_sync_drive_validation_key(
    refresh_token: Option<&str>,
    client_id: Option<&str>,
    client_secret: Option<&str>,
) -> Option<&'static str> {
    if refresh_token.unwrap_or_default().trim().is_empty() {
        Some("settings.driveRefreshTokenRequired")
    } else if client_id.unwrap_or_default().trim().is_empty() {
        Some("settings.driveClientIdRequired")
    } else if client_secret.unwrap_or_default().trim().is_empty() {
        Some("settings.driveClientSecretRequired")
    } else {
        None
    }
}

fn cloud_sync_action_button(
    palette: ThemePalette,
    id: &'static str,
    label: impl Into<SharedString>,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let label: SharedString = label.into();
    let hover = palette.hover;
    div()
        .id(id)
        .h(px(28.))
        .px_3()
        .flex()
        .items_center()
        .rounded_sm()
        .border_1()
        .border_color(rgb(palette.border))
        .bg(rgb(palette.surface_elevated))
        .text_color(rgb(palette.text))
        .text_xs()
        .opacity(if enabled { 1.0 } else { 0.45 })
        .when(enabled, |this| {
            this.cursor_pointer()
                .hover(move |this| this.bg(rgb(hover)))
                .on_click(on_click)
        })
        .child(label)
}

fn cloud_sync_status_item(
    palette: ThemePalette,
    label: impl Into<SharedString>,
    value: String,
) -> impl IntoElement {
    let label: SharedString = label.into();
    div()
        .rounded_md()
        .border_1()
        .border_color(rgb(palette.border))
        .bg(rgb(palette.input))
        .px_3()
        .py_2()
        .min_w_0()
        .child(
            div()
                .text_size(px(10.))
                .font_weight(FontWeight(600.))
                .text_color(rgb(palette.text_muted))
                .child(label),
        )
        .child(
            div()
                .mt_1()
                .text_size(px(12.))
                .text_color(rgb(palette.text))
                .overflow_hidden()
                .child(value),
        )
}

#[cfg(test)]
mod tests {
    use super::{CloudSyncLiveState, cloud_sync_state_i18n_key};

    #[test]
    fn cloud_sync_badge_prioritizes_availability_conflict_and_live_state() {
        assert_eq!(
            cloud_sync_state_i18n_key(false, false, CloudSyncLiveState::Idle, None),
            "settings.syncState.disabled"
        );
        assert_eq!(
            cloud_sync_state_i18n_key(true, true, CloudSyncLiveState::Failed, Some("success")),
            "settings.syncState.conflict"
        );
        assert_eq!(
            cloud_sync_state_i18n_key(true, false, CloudSyncLiveState::Running, None),
            "settings.syncState.running"
        );
        assert_eq!(
            cloud_sync_state_i18n_key(true, false, CloudSyncLiveState::Failed, Some("success"),),
            "settings.syncState.failed"
        );
        assert_eq!(
            cloud_sync_state_i18n_key(true, false, CloudSyncLiveState::Success, Some("failed"),),
            "settings.syncState.success"
        );
        assert_eq!(
            cloud_sync_state_i18n_key(true, false, CloudSyncLiveState::Idle, Some("success"),),
            "settings.syncState.success"
        );
    }
}
