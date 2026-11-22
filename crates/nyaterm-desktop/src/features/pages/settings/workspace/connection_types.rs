use gpui::{Context, IntoElement, div, prelude::*};
use nyaterm_ui::{NyaButton, NyaButtonVariant};
use rust_i18n::t;

use crate::features::pages::settings::panel::SettingsPanel;
use crate::features::pages::settings::{settings_form_section, settings_switch_with_enabled};
use crate::models::ConnectionKindTab;

impl SettingsPanel {
    pub(super) fn connection_types_settings_section(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let layout = &self.settings.summary().connection_type_layout;
        let kinds = ConnectionKindTab::editor_types(layout, true, &[]);
        let catalog = ConnectionKindTab::catalog_ids();
        let visible = layout.visible(&catalog);
        let mut rows = div().flex().flex_col().gap_2();
        for (index, kind) in kinds.iter().copied().enumerate() {
            let checked = visible.contains(&kind.persistence_id());
            let enabled = !checked || visible.len() > 1;
            rows = rows.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(kind.display_label().into_owned()),
                    )
                    .child(
                        NyaButton::new(
                            format!("connection-type-up-{}", kind.persistence_id()),
                            t!("settings.startupMoveUp"),
                        )
                        .variant(NyaButtonVariant::Ghost)
                        .disabled(index == 0)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.with_app(cx, |app, cx| app.move_connection_type(kind, false, cx));
                        })),
                    )
                    .child(
                        NyaButton::new(
                            format!("connection-type-down-{}", kind.persistence_id()),
                            t!("settings.startupMoveDown"),
                        )
                        .variant(NyaButtonVariant::Ghost)
                        .disabled(index + 1 == kinds.len())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.with_app(cx, |app, cx| app.move_connection_type(kind, true, cx));
                        })),
                    )
                    .child(settings_switch_with_enabled(
                        palette,
                        format!("connection-type-visible-{}", kind.persistence_id()),
                        checked,
                        enabled,
                        cx.listener(move |this, _, _, cx| {
                            this.with_app(cx, |app, cx| {
                                app.set_connection_type_visible(kind, !checked, cx)
                            });
                        }),
                    )),
            );
        }
        rows = rows.child(
            div().flex().justify_end().child(
                NyaButton::new(
                    "connection-types-reset",
                    t!("settings.connectionTypesReset"),
                )
                .variant(NyaButtonVariant::Ghost)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.with_app(cx, |app, cx| app.reset_connection_type_layout(cx));
                })),
            ),
        );
        settings_form_section(
            palette,
            Some(t!("settings.connectionTypes")),
            Some(t!("settings.connectionTypesDescription")),
            rows,
        )
    }
}
