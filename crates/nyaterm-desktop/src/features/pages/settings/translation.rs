use rust_i18n::t;

use gpui::{AnyElement, Context, FontWeight, IntoElement, SharedString, div, prelude::*, px, rgb};
use nyaterm_ui::NyaSelectOption;

use crate::features::pages::settings::panel::SettingsPanel;
use crate::models::TranslateInputField;
use crate::widgets::{small_button, status_pill};

use super::settings_form_section;

impl SettingsPanel {
    fn translation_input(
        &mut self,
        _id: &'static str,
        label: impl Into<SharedString>,
        value: String,
        field: TranslateInputField,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let label: SharedString = label.into();
        let _ = (value, cx);
        self.existing_text_input_field(
            format!("translation.input.{}", field.input_key()),
            label,
            field == TranslateInputField::Text,
        )
    }

    pub(in crate::features) fn translation_settings_section(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let (translation_settings, secret_draft) = self.translation.settings_draft_snapshot();
        let deepl_key_value = secret_draft.deepl_api_key.expose_secret().to_owned();
        let baidu_app_id_value = translation_settings.baidu_app_id.clone();
        let baidu_key_value = secret_draft.baidu_app_key.expose_secret().to_owned();
        let ali_app_id_value = translation_settings.ali_app_id.clone();
        let ali_key_value = secret_draft.ali_app_key.expose_secret().to_owned();
        let youdao_app_id_value = translation_settings.youdao_app_id.clone();
        let youdao_key_value = secret_draft.youdao_app_key.expose_secret().to_owned();

        let deepl_configured = !translation_settings.deepl_api_key.trim().is_empty()
            || !secret_draft.deepl_api_key.is_empty();
        let baidu_configured = !translation_settings.baidu_app_id.trim().is_empty()
            && (!translation_settings.baidu_app_key.trim().is_empty()
                || !secret_draft.baidu_app_key.is_empty());
        let ali_configured = !translation_settings.ali_app_id.trim().is_empty()
            && (!translation_settings.ali_app_key.trim().is_empty()
                || !secret_draft.ali_app_key.is_empty());
        let youdao_configured = !translation_settings.youdao_app_id.trim().is_empty()
            && (!translation_settings.youdao_app_key.trim().is_empty()
                || !secret_draft.youdao_app_key.is_empty());

        let target_language_label = t!("settings.targetLanguage");
        let providers_label = t!("settings.translationProviders");
        let providers_desc = t!("settings.translationProvidersDesc");
        let no_key_label = t!("settings.noKeyRequired");
        let configured_label = t!("settings.configured");
        let not_configured_label = t!("settings.notConfigured");
        let api_key_label = t!("settings.apiKey");
        let app_id_label = t!("settings.appId");
        let app_key_label = t!("settings.appKey");
        let remove_label = t!("common.remove");

        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(settings_form_section(
                palette,
                None,
                None,
                self.settings_select_field(
                    "settings.translation.target-language",
                    target_language_label,
                    None,
                    (
                        translation_target_languages()
                            .iter()
                            .map(|(code, label)| NyaSelectOption::new(*code, *label))
                            .collect(),
                        translation_settings.target_language.clone(),
                        false,
                    ),
                    cx,
                ),
            ))
            .child(settings_form_section(
                palette,
                Some(providers_label),
                Some(providers_desc),
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(translation_provider_card(
                        palette,
                        t!("translation.google"),
                        no_key_label.clone(),
                        true,
                        true,
                        div(),
                    ))
                    .child(translation_provider_card(
                        palette,
                        t!("translation.microsoft"),
                        no_key_label,
                        true,
                        true,
                        div(),
                    ))
                    .child(translation_provider_card(
                        palette,
                        t!("translation.deepl"),
                        if deepl_configured {
                            configured_label.clone()
                        } else {
                            not_configured_label.clone()
                        },
                        deepl_configured,
                        false,
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(self.translation_input(
                                "translation-deepl-api-key",
                                api_key_label,
                                deepl_key_value,
                                TranslateInputField::DeeplApiKey,
                                cx,
                            ))
                            .child(small_button(
                                palette,
                                "translation-clear-deepl",
                                remove_label.clone(),
                                cx.listener(|this, _, _, cx| {
                                    this.clear_translation_secret("deepl", cx);
                                }),
                            )),
                    ))
                    .child(translation_provider_card(
                        palette,
                        t!("translation.baidu"),
                        if baidu_configured {
                            configured_label.clone()
                        } else {
                            not_configured_label.clone()
                        },
                        baidu_configured,
                        false,
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .grid()
                                    .grid_cols(2)
                                    .gap_2()
                                    .child(self.translation_input(
                                        "translation-baidu-app-id",
                                        app_id_label.clone(),
                                        baidu_app_id_value,
                                        TranslateInputField::BaiduAppId,
                                        cx,
                                    ))
                                    .child(self.translation_input(
                                        "translation-baidu-app-key",
                                        app_key_label.clone(),
                                        baidu_key_value,
                                        TranslateInputField::BaiduAppKey,
                                        cx,
                                    )),
                            )
                            .child(small_button(
                                palette,
                                "translation-clear-baidu",
                                remove_label.clone(),
                                cx.listener(|this, _, _, cx| {
                                    this.clear_translation_secret("baidu", cx);
                                }),
                            )),
                    ))
                    .child(translation_provider_card(
                        palette,
                        t!("translation.ali"),
                        if ali_configured {
                            configured_label.clone()
                        } else {
                            not_configured_label.clone()
                        },
                        ali_configured,
                        false,
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .grid()
                                    .grid_cols(2)
                                    .gap_2()
                                    .child(self.translation_input(
                                        "translation-ali-app-id",
                                        app_id_label.clone(),
                                        ali_app_id_value,
                                        TranslateInputField::AliAppId,
                                        cx,
                                    ))
                                    .child(self.translation_input(
                                        "translation-ali-app-key",
                                        app_key_label.clone(),
                                        ali_key_value,
                                        TranslateInputField::AliAppKey,
                                        cx,
                                    )),
                            )
                            .child(small_button(
                                palette,
                                "translation-clear-ali",
                                remove_label.clone(),
                                cx.listener(|this, _, _, cx| {
                                    this.clear_translation_secret("ali", cx);
                                }),
                            )),
                    ))
                    .child(translation_provider_card(
                        palette,
                        t!("translation.youdao"),
                        if youdao_configured {
                            configured_label
                        } else {
                            not_configured_label
                        },
                        youdao_configured,
                        false,
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .grid()
                                    .grid_cols(2)
                                    .gap_2()
                                    .child(self.translation_input(
                                        "translation-youdao-app-id",
                                        app_id_label,
                                        youdao_app_id_value,
                                        TranslateInputField::YoudaoAppId,
                                        cx,
                                    ))
                                    .child(self.translation_input(
                                        "translation-youdao-app-key",
                                        app_key_label,
                                        youdao_key_value,
                                        TranslateInputField::YoudaoAppKey,
                                        cx,
                                    )),
                            )
                            .child(small_button(
                                palette,
                                "translation-clear-youdao",
                                remove_label,
                                cx.listener(|this, _, _, cx| {
                                    this.clear_translation_secret("youdao", cx);
                                }),
                            )),
                    )),
            ))
    }
}

fn translation_provider_card(
    palette: crate::theme::ThemePalette,
    title: impl Into<SharedString>,
    status_label: impl Into<SharedString>,
    ok: bool,
    free: bool,
    body: impl IntoElement,
) -> impl IntoElement {
    let title: SharedString = title.into();
    let status_label: SharedString = status_label.into();
    let (fg, bg) = if free {
        (palette.link, palette.hover)
    } else if ok {
        (palette.success, 0x12261c)
    } else {
        (palette.text_muted, palette.border)
    };
    div()
        .rounded_md()
        .border_1()
        .border_color(rgb(palette.border))
        .bg(rgb(palette.input))
        .p_3()
        .flex()
        .flex_col()
        .gap_3()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    div()
                        .text_size(px(12.))
                        .font_weight(FontWeight(600.))
                        .text_color(rgb(palette.text))
                        .child(title),
                )
                .child(status_pill(status_label, rgb(fg), rgb(bg))),
        )
        .when(!free, |this| this.child(body))
}

fn translation_target_languages() -> &'static [(&'static str, &'static str)] {
    &[
        ("zh-CN", "中文 (简体)"),
        ("zh-TW", "中文 (繁體)"),
        ("en", "English"),
        ("ja", "日本語"),
        ("ko", "한국어"),
        ("fr", "Français"),
        ("de", "Deutsch"),
        ("es", "Español"),
        ("pt", "Português (Brasil)"),
        ("ru", "Русский"),
        ("it", "Italiano"),
        ("ar", "العربية"),
        ("th", "ไทย"),
        ("vi", "Tiếng Việt"),
    ]
}
