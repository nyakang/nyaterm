use rust_i18n::t;

use gpui::{Context, IntoElement, SharedString, div, prelude::*};

use crate::features::pages::settings::panel::SettingsPanel;

use super::super::{
    settings_form_row, settings_form_section, settings_input_control, settings_switch,
};

impl SettingsPanel {
    pub(in crate::features::pages::settings) fn transfer_advanced_settings_section(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let _permissions = self
            .settings
            .summary()
            .transfer_default_file_permissions
            .clone();
        let permissions_input = self
            .existing_text_input_box("settings.transfer.default-permissions", false)
            .into_any_element();

        div().flex().flex_col().gap_3().child(settings_form_section(
            palette,
            None,
            None,
            div()
                .flex()
                .flex_col()
                .gap_3()
                .child(settings_form_row(
                    palette,
                    t!("settings.downloadConcurrentTasks"),
                    Some(SharedString::from(t!(
                        "settings.downloadConcurrentTasksDesc"
                    ))),
                    self.existing_number_input_box("settings.number.transfer-download-threads"),
                ))
                .child(settings_form_row(
                    palette,
                    t!("settings.uploadConcurrentTasks"),
                    Some(SharedString::from(t!("settings.uploadConcurrentTasksDesc"))),
                    self.existing_number_input_box("settings.number.transfer-upload-threads"),
                ))
                .child(settings_form_row(
                    palette,
                    t!("settings.maxTransferRetries"),
                    Some(SharedString::from(t!("settings.maxTransferRetriesDesc"))),
                    self.existing_number_input_box("settings.number.transfer-max-retries"),
                ))
                .child(settings_form_row(
                    palette,
                    t!("settings.transferBufferSize"),
                    Some(SharedString::from(t!("settings.transferBufferSizeDesc"))),
                    self.existing_number_input_box("settings.number.transfer-buffer-size"),
                ))
                .child(settings_form_row(
                    palette,
                    t!("settings.preserveTimestamps"),
                    None,
                    settings_switch(
                        palette,
                        "settings-transfer-preserve-timestamps",
                        self.settings.summary().transfer_preserve_timestamps,
                        cx.listener(|this, _, _, cx| {
                            this.toggle_transfer_preserve_timestamps(cx);
                        }),
                    ),
                ))
                .child(settings_form_row(
                    palette,
                    t!("settings.resumeBrokenTransfer"),
                    None,
                    settings_switch(
                        palette,
                        "settings-transfer-resume-broken",
                        self.settings.summary().transfer_resume_broken_transfer,
                        cx.listener(|this, _, _, cx| {
                            this.toggle_transfer_resume_broken(cx);
                        }),
                    ),
                ))
                .child(settings_form_row(
                    palette,
                    t!("settings.defaultFilePermissions"),
                    Some(SharedString::from(t!(
                        "settings.defaultFilePermissionsDesc"
                    ))),
                    settings_input_control(permissions_input),
                )),
        ))
    }
}
