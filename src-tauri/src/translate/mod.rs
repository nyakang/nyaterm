mod ali;
mod baidu;
mod deepl;
mod google;
mod microsoft;
mod youdao;

use crate::config::TranslationSettings;
use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranslateResult {
    pub original: String,
    pub translated: String,
    pub detected_language: String,
    pub provider: String,
}

pub async fn translate(
    provider: &str,
    text: &str,
    target_lang: &str,
    settings: &TranslationSettings,
) -> AppResult<TranslateResult> {
    let result = match provider {
        "google" => google::translate(text, target_lang).await,
        "microsoft" => microsoft::translate(text, target_lang).await,
        "deepl" => deepl::translate(text, target_lang, &settings.deepl_api_key).await,
        "baidu" => {
            baidu::translate(text, target_lang, &settings.baidu_app_id, &settings.baidu_app_key)
                .await
        }
        "ali" => {
            ali::translate(text, target_lang, &settings.ali_app_id, &settings.ali_app_key).await
        }
        "youdao" => {
            youdao::translate(
                text,
                target_lang,
                &settings.youdao_app_id,
                &settings.youdao_app_key,
            )
            .await
        }
        _ => Err(AppError::Translation(format!(
            "Unknown provider: {provider}"
        ))),
    };

    result.map(|mut r| {
        r.original = text.to_string();
        r.provider = provider.to_string();
        r
    })
}

#[tauri::command]
pub async fn translate_text(
    app: tauri::AppHandle,
    provider: String,
    text: String,
    target_language: String,
) -> AppResult<TranslateResult> {
    let settings = crate::config::load_app_settings(&app)?;
    let target = if target_language.is_empty() {
        &settings.translation.target_language
    } else {
        &target_language
    };
    translate(&provider, &text, target, &settings.translation).await
}
