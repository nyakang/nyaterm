use super::super::settings_switch;
use super::{ai_card, ai_field};
use crate::features::pages::settings::panel::SettingsPanel;
use crate::models::{AiActionEditorField, AiActionListKind};
use gpui::{AnyElement, Context, IntoElement, div, prelude::*, px, rgb};
use nyaterm_ui::{NyaButton, NyaIconButton};
use rust_i18n::t;

impl SettingsPanel {
    pub(in crate::features) fn ai_rules_settings_section(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let limit = ai_field(
            palette,
            format!("{} (MiB)", t!("ai.maxAiFileSize")),
            Some(t!("ai.maxAiFileSizeDesc").into()),
            self.existing_number_input_box("ai.number.file-size-mb"),
        );
        let terminal = self.ai_action_editor(AiActionListKind::Terminal, cx);
        let file = self.ai_action_editor(AiActionListKind::File, cx);
        div()
            .flex()
            .flex_col()
            .gap_5()
            .child(ai_card(palette, div().child(t!("ai.rules")), div(), limit))
            .child(terminal)
            .child(file)
    }

    fn ai_action_editor(&mut self, kind: AiActionListKind, cx: &mut Context<Self>) -> AnyElement {
        let palette = self.theme_palette();
        let (title, actions) = match kind {
            AiActionListKind::Terminal => (
                t!("ai.terminalActions"),
                self.ai.settings_config().terminal_ai_actions.clone(),
            ),
            AiActionListKind::File => (
                t!("ai.fileActions"),
                self.ai.settings_config().file_ai_actions.clone(),
            ),
        };
        let add = NyaButton::new(format!("ai-action-add-{kind:?}"), t!("common.add"))
            .small()
            .icon("icons/settings/plus.svg")
            .on_click(
                cx.listener(move |panel, _, window, cx| panel.add_ai_action(kind, window, cx)),
            );
        let rows = div()
            .flex()
            .flex_col()
            .gap_4()
            .children(actions.into_iter().map(|action| {
                let toggle_id = action.id.clone();
                let delete_id = action.id.clone();
                let name = self.existing_text_input_box(
                    Self::ai_action_text_input_id(kind, &action.id, AiActionEditorField::Name),
                    false,
                );
                let prompt = self.existing_text_input_box(
                    Self::ai_action_text_input_id(kind, &action.id, AiActionEditorField::Prompt),
                    true,
                );
                div()
                    .id(format!("ai-action-{}", action.id))
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(palette.border))
                    .bg(rgb(palette.bg))
                    .p_4()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().min_w_0().flex_1().child(name))
                            .child(settings_switch(
                                palette,
                                format!("ai-action-enabled-{}", action.id),
                                action.enabled,
                                cx.listener(move |panel, _, _, cx| {
                                    panel.toggle_ai_action_enabled(kind, toggle_id.clone(), cx)
                                }),
                            ))
                            .child(
                                NyaIconButton::new(
                                    format!("ai-action-delete-{}", action.id),
                                    "icons/settings/trash.svg",
                                )
                                .form_control()
                                .tooltip(t!("common.delete"))
                                .on_click(cx.listener(
                                    move |panel, _, _, cx| {
                                        panel.remove_ai_action(kind, delete_id.clone(), cx)
                                    },
                                )),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(palette.text_muted))
                            .child(t!("ai.actionPrompt")),
                    )
                    .child(div().w_full().min_w_0().child(prompt))
            }));
        ai_card(palette, div().child(title), add, rows).into_any_element()
    }
}
