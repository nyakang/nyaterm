use rust_i18n::t;

use gpui::{Context, IntoElement, SharedString, div, prelude::*};
use nyaterm_ui::NyaSelectOption;

use crate::features::{pages::settings::panel::SettingsPanel, transfers::duplicate_policy_label};

use super::super::{
    settings_form_row, settings_form_section, settings_input_action_control, settings_switch,
};

impl SettingsPanel {
    pub(in crate::features) fn transfer_settings_section(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let download_path_input = self
            .existing_text_input_box("settings.transfer.download-path", false)
            .into_any_element();
        let policy = self.transfer.duplicate_policy();
        let selected_policy = duplicate_policy_label(policy);

        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(settings_form_section(
                palette,
                None,
                None,
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(settings_form_row(
                        palette,
                        t!("settings.downloadPath"),
                        Some(SharedString::from(t!("settings.downloadPathDesc"))),
                        settings_input_action_control(
                            260.,
                            download_path_input,
                            nyaterm_ui::NyaButton::new(
                                "transfer-browse-download",
                                t!("settings.browse"),
                            )
                            .form_control()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.prompt_transfer_download_path_setting(cx);
                            })),
                        ),
                    ))
                    .child(settings_form_row(
                        palette,
                        t!("settings.askSaveLocation"),
                        None,
                        settings_switch(
                            palette,
                            "transfer-ask-save",
                            self.settings.summary().transfer_ask_save_location,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_transfer_ask_save_location(cx);
                            }),
                        ),
                    ))
                    .child(settings_form_row(
                        palette,
                        t!("settings.fileManagerAutoCollapse"),
                        Some(SharedString::from(t!(
                            "settings.fileManagerAutoCollapseDesc"
                        ))),
                        settings_switch(
                            palette,
                            "file-manager-auto-collapse",
                            self.settings
                                .summary()
                                .ui_file_manager
                                .auto_collapse_sidebar,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_file_manager_auto_collapse(cx);
                            }),
                        ),
                    ))
                    .child(settings_form_row(
                        palette,
                        t!("settings.duplicateStrategy"),
                        Some(SharedString::from(t!("settings.duplicateStrategyDesc"))),
                        self.settings_select_control(
                            "settings.transfer.duplicate-strategy",
                            vec![
                                NyaSelectOption::new("overwrite", t!("settings.strategyOverwrite")),
                                NyaSelectOption::new("skip", t!("settings.strategySkip")),
                                NyaSelectOption::new("rename", t!("settings.strategyRename")),
                                NyaSelectOption::new("ask", t!("settings.strategyAsk")),
                            ],
                            selected_policy,
                            false,
                            cx,
                        ),
                    ))
                    .child(self.transfer_editor_settings_rows(cx))
                    .child(settings_form_row(
                        palette,
                        t!("settings.externalEditorAutoUpload"),
                        Some(SharedString::from(t!(
                            "settings.externalEditorAutoUploadDesc"
                        ))),
                        settings_switch(
                            palette,
                            "transfer-external-editor-auto-upload",
                            self.settings.summary().transfer_external_editor_auto_upload,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_transfer_external_editor_auto_upload(cx);
                            }),
                        ),
                    )),
            ))
            .child(self.transfer_advanced_settings_section(cx))
    }
}
