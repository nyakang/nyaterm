use gpui::{AnyElement, Context, IntoElement, div, prelude::*};
use rust_i18n::t;

use crate::features::pages::settings::panel::SettingsPanel;
use crate::features::pages::settings::settings_form_section;
use crate::widgets::small_button;

impl SettingsPanel {
    pub(super) fn cloud_sync_v3_section(
        &mut self,
        actions_enabled: bool,
        form_enabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = self.theme_palette();
        let selected =
            crate::features::sync::effective_protocol_version(self.cloud_sync.settings()) == 4;
        let capable = nyaterm_sync::protocol::ProviderCapabilities::for_provider(
            &self.cloud_sync.settings().provider,
        )
        .is_some_and(|capabilities| capabilities.supports_v4());
        let preview = self.cloud_sync.v4_preview.clone();
        let mut content = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(div().text_sm().child(t!("settings.syncV3Description")))
            .child(div().text_sm().child(t!("settings.syncFixedScope")))
            .child(div().text_sm().child(t!("settings.syncFixedScopeDesc")));
        if !selected {
            if capable && form_enabled {
                content = content.child(small_button(
                    palette,
                    "sync-v3-select",
                    t!("settings.syncV3Select"),
                    cx.listener(|this, _, _, cx| {
                        this.with_app(cx, |app, cx| app.select_cloud_sync_v3(cx));
                    }),
                ));
            }
        } else {
            if actions_enabled && capable {
                content = content.child(small_button(
                    palette,
                    "sync-v3-preview",
                    t!("settings.syncV3Preview"),
                    cx.listener(|this, _, _, cx| {
                        this.with_app(cx, |app, cx| app.run_cloud_sync_v3_preview(cx));
                    }),
                ));
            }
            if let Some(preview) = preview {
                content = content.child(div().text_sm().child(t!(
                    "settings.syncV3PreviewCounts",
                    resources = preview.resources,
                    conflicts = preview.conflicts.len()
                )));
                if preview.migration {
                    content =
                        content.child(div().text_sm().child(t!("settings.syncV3MigrationNotice")));
                }
                if !preview.deferred.is_empty() {
                    content = content.child(div().text_sm().child(t!(
                        "settings.syncV3Deferred",
                        count = preview.deferred.len()
                    )));
                    for field in &preview.deferred {
                        content = content.child(div().text_sm().child(field.clone()));
                    }
                }
                for conflict in &preview.conflicts {
                    let conflict_index = conflict.index;
                    let mut row = div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(div().text_sm().child(conflict.field.clone()));
                    if actions_enabled {
                        for (candidate_index, label) in conflict.choices.iter().enumerate() {
                            row = row.child(small_button(
                                palette,
                                format!("sync-v3-choice-{conflict_index}-{candidate_index}"),
                                label.clone(),
                                cx.listener(move |this, _, _, cx| {
                                    this.with_app(cx, |app, cx| {
                                        app.resolve_cloud_sync_v3_preview(
                                            conflict_index,
                                            candidate_index,
                                            cx,
                                        )
                                    });
                                }),
                            ));
                        }
                    }
                    content = content.child(row);
                }
                if actions_enabled && capable {
                    content = content.child(small_button(
                        palette,
                        "sync-v3-apply",
                        t!("settings.syncV3Apply"),
                        cx.listener(|this, _, _, cx| {
                            this.with_app(cx, |app, cx| app.apply_cloud_sync_v3_preview(cx));
                        }),
                    ));
                }
            }
        }
        settings_form_section(palette, Some("Cloud Sync V3".into()), None, content)
            .into_any_element()
    }
}
