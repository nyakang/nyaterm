use rust_i18n::t;

use gpui::Context;

use crate::features::NyaTermApp;
use crate::models::{CloudSyncInputField, SettingsTab};

impl NyaTermApp {
    pub(in crate::features) fn update_cloud_sync_provider(
        &mut self,
        provider: &str,
        cx: &mut Context<Self>,
    ) {
        if !self.cloud_sync_form_enabled() {
            return;
        }
        let provider = match provider {
            "s3" | "gitee_snippet" | "github_gist" | "google_drive" | "onedrive"
            | "aliyun_drive" => provider,
            _ => "webdav",
        };
        if provider != "github_gist" && self.cloud_sync.github_auth().pending {
            self.cancel_github_gist_auth(cx);
        }
        self.cloud_sync.select_provider(provider);
        self.request_settings_panel_refresh(cx);
        cx.notify();
    }

    pub(in crate::features) fn toggle_cloud_sync_enabled(&mut self, cx: &mut Context<Self>) {
        if !self.cloud_sync.settings().enabled
            && (!self.settings.master_password().enabled
                || (!self.settings.summary().has_master_password
                    && self.settings.master_password().draft.is_empty()))
        {
            self.focus_settings_tab(SettingsTab::Security, cx);
            self.cloud_sync
                .set_status(t!("settings.syncMasterPasswordRequired"));
            self.shell.set_status(self.cloud_sync.status().to_string());
            cx.notify();
            return;
        }
        self.cloud_sync.toggle_enabled();
        self.request_settings_panel_refresh(cx);
        cx.notify();
    }

    pub(in crate::features) fn toggle_s3_virtual_host_style(&mut self, cx: &mut Context<Self>) {
        if !self.cloud_sync_form_enabled() {
            return;
        }
        self.cloud_sync.toggle_s3_virtual_host_style();
        self.request_settings_panel_refresh(cx);
        cx.notify();
    }

    pub(in crate::features) fn toggle_cloud_sync_auto_check(&mut self, cx: &mut Context<Self>) {
        if !self.cloud_sync_form_enabled() || !self.cloud_sync.settings().enabled {
            return;
        }
        self.cloud_sync.toggle_auto_check();
        self.request_settings_panel_refresh(cx);
        cx.notify();
    }

    pub(in crate::features) fn toggle_cloud_sync_auto_push(&mut self, cx: &mut Context<Self>) {
        if !self.cloud_sync_form_enabled() || !self.cloud_sync.settings().enabled {
            return;
        }
        self.cloud_sync.toggle_auto_push();
        self.request_settings_panel_refresh(cx);
        cx.notify();
    }

    pub(in crate::features) fn toggle_cloud_sync_auto_pull_remote_changes(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        if !self.cloud_sync_form_enabled() || !self.cloud_sync.settings().enabled {
            return;
        }
        self.cloud_sync.toggle_auto_pull_remote_changes();
        self.request_settings_panel_refresh(cx);
        cx.notify();
    }

    pub(in crate::features) fn set_cloud_sync_debounce(
        &mut self,
        value: u64,
        cx: &mut Context<Self>,
    ) {
        if !self.cloud_sync_form_enabled()
            || !self.cloud_sync.settings().enabled
            || !self.cloud_sync.settings().auto_push_on_change
        {
            return;
        }
        self.cloud_sync.set_debounce(value);
        self.request_settings_panel_refresh(cx);
        cx.notify();
    }

    /// Apply an edit from one of the cloud sync inputs.
    ///
    /// Field edits are not gated on the master password the way the enable switch
    /// is: the provider fields stay editable while the form is dimmed so a config
    /// can be prepared before any password exists. Dropping them here would let a
    /// keystroke disappear silently and leave the settings draft clean, which looks
    /// like an unclickable apply button. `pending_settings_validation_error` reports
    /// the missing master password instead.
    pub(in crate::features) fn apply_cloud_sync_input(
        &mut self,
        field: CloudSyncInputField,
        text: String,
        cx: &mut Context<Self>,
    ) {
        if self.cloud_sync.apply_input(field, text) {
            self.request_settings_panel_refresh(cx);
            cx.notify();
        }
    }

    pub(in crate::features) fn cloud_sync_form_enabled(&self) -> bool {
        self.settings.master_password().enabled
            && (self.settings.summary().has_master_password
                || !self.settings.master_password().draft.is_empty())
    }
}
