use rust_i18n::t;

use gpui::{Context, IntoElement, SharedString, div, prelude::*};
use nyaterm_ui::NyaSelectOption;

use crate::features::pages::settings::panel::SettingsPanel;

use super::super::{settings_form_row, settings_input_action_control};

impl SettingsPanel {
    pub(in crate::features::pages::settings) fn transfer_editor_settings_rows(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let editor_type = self.settings.summary().transfer_editor_type.clone();
        let default_editor_input = self
            .existing_text_input_box("settings.transfer.default-editor", false)
            .into_any_element();
        let selected_editor_type = if editor_type == "internal" {
            "internal"
        } else {
            "external"
        };

        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(settings_form_row(
                palette,
                t!("settings.editorType"),
                None,
                self.settings_select_control(
                    "settings.transfer.editor-type",
                    vec![
                        NyaSelectOption::new("external", t!("settings.editorTypeExternal")),
                        NyaSelectOption::new("internal", t!("settings.editorTypeInternal")),
                    ],
                    selected_editor_type,
                    false,
                    cx,
                ),
            ))
            .when(editor_type == "internal", |this| {
                this.child(settings_form_row(
                    palette,
                    t!("settings.internalEditorDisplay"),
                    None,
                    self.settings_select_control(
                        "settings.transfer.internal-editor-display",
                        vec![
                            NyaSelectOption::new(
                                "workspace",
                                t!("settings.internalEditorWorkspace"),
                            ),
                            NyaSelectOption::new("window", t!("settings.internalEditorWindow")),
                        ],
                        self.settings
                            .summary()
                            .transfer_internal_editor_display
                            .compat_value(),
                        false,
                        cx,
                    ),
                ))
                .child(settings_form_row(
                    palette,
                    t!("settings.internalEditorFontSize"),
                    None,
                    self.existing_number_input_box(
                        "settings.number.transfer-internal-editor-font-size",
                    ),
                ))
            })
            .when(editor_type == "external", |this| {
                this.child(settings_form_row(
                    palette,
                    t!("settings.defaultEditor"),
                    Some(SharedString::from(t!("settings.defaultEditorDesc"))),
                    settings_input_action_control(
                        260.,
                        default_editor_input,
                        nyaterm_ui::NyaButton::new(
                            "settings-transfer-editor-browse",
                            t!("settings.browse"),
                        )
                        .form_control()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.prompt_transfer_default_editor_setting(cx);
                        })),
                    ),
                ))
            })
    }
}
