use gpui::{AnyElement, Context, IntoElement, SharedString, div, prelude::*};
use rust_i18n::t;

use crate::features::pages::settings::panel::SettingsPanel;
use crate::features::pages::settings::{
    settings_form_row, settings_form_section, settings_switch_with_enabled,
};
use crate::widgets::small_button;

impl SettingsPanel {
    pub(super) fn cloud_sync_v3_section(
        &mut self,
        actions_enabled: bool,
        form_enabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = self.theme_palette();
        let selected = self.cloud_sync.settings().protocol_version == 4;
        let capable = nyaterm_sync::protocol::ProviderCapabilities::for_provider(
            &self.cloud_sync.settings().provider,
        )
        .is_some_and(|capabilities| capabilities.supports_v4());
        let preview = self.cloud_sync.v4_preview.clone();
        let mut content = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(div().text_sm().child(t!("settings.syncV3Description")));
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
            content = content.child(settings_form_row(
                palette,
                t!("settings.syncV3Preferences"),
                Some(SharedString::from(t!("settings.syncV3PreferencesDesc"))),
                settings_switch_with_enabled(
                    palette,
                    "sync-v3-preferences",
                    self.cloud_sync.settings().v4_scope.preferences,
                    form_enabled,
                    cx.listener(|this, _, _, cx| {
                        this.with_app(cx, |app, cx| app.toggle_cloud_sync_v3_preferences(cx));
                    }),
                ),
            ));
            content = content.child(settings_form_row(
                palette,
                t!("settings.syncV3Keybindings"),
                Some(SharedString::from(t!("settings.syncV3KeybindingsDesc"))),
                settings_switch_with_enabled(
                    palette,
                    "sync-v3-keybindings",
                    self.cloud_sync.settings().v4_scope.keybindings,
                    form_enabled,
                    cx.listener(|this, _, _, cx| {
                        this.with_app(cx, |app, cx| app.toggle_cloud_sync_v3_keybindings(cx));
                    }),
                ),
            ));
            content = content.child(settings_form_row(
                palette,
                t!("settings.syncV3Ai"),
                Some(SharedString::from(t!("settings.syncV3AiDesc"))),
                settings_switch_with_enabled(
                    palette,
                    "sync-v3-ai",
                    self.cloud_sync.settings().v4_scope.ai,
                    form_enabled,
                    cx.listener(|this, _, _, cx| {
                        this.with_app(cx, |app, cx| app.toggle_cloud_sync_v3_ai(cx));
                    }),
                ),
            ));
            content = content.child(settings_form_row(
                palette,
                t!("settings.syncV3Trust"),
                Some(SharedString::from(t!("settings.syncV3TrustDesc"))),
                settings_switch_with_enabled(
                    palette,
                    "sync-v3-trust",
                    self.cloud_sync.settings().v4_scope.trusted_hosts,
                    form_enabled,
                    cx.listener(|this, _, _, cx| {
                        this.with_app(cx, |app, cx| app.toggle_cloud_sync_v3_trust(cx));
                    }),
                ),
            ));
            if actions_enabled && capable {
                content = content.child(settings_form_row(
                    palette,
                    t!("settings.syncV3Secrets"),
                    Some(SharedString::from(t!("settings.syncV3SecretsDesc"))),
                    settings_switch_with_enabled(
                        palette,
                        "sync-v3-secrets",
                        self.cloud_sync.settings().v4_scope.secrets,
                        form_enabled,
                        cx.listener(|this, _, _, cx| {
                            this.with_app(cx, |app, cx| app.toggle_cloud_sync_v3_secrets(cx));
                        }),
                    ),
                ));
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
                for (index, review) in preview.trust.iter().enumerate() {
                    content = content.child(div().text_sm().child(review.description.clone()));
                    if actions_enabled {
                        let key = review.key.clone();
                        let token = review.token.clone();
                        content = content.child(small_button(
                            palette,
                            format!("sync-v3-trust-{index}"),
                            t!("settings.syncV3TrustConfirm"),
                            cx.listener(move |this, _, _, cx| {
                                this.with_app(cx, |app, cx| {
                                    app.confirm_cloud_sync_v3_trust(key.clone(), token.clone(), cx)
                                });
                            }),
                        ));
                    }
                }
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
