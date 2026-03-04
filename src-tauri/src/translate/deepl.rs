use crate::error::{AppError, AppResult};
use serde::Deserialize;

use super::TranslateResult;

#[derive(Deserialize)]
struct DeeplResponse {
    translations: Vec<DeeplTranslation>,
}

#[derive(Deserialize)]
struct DeeplTranslation {
    text: String,
    detected_source_language: Option<String>,
}

pub async fn translate(text: &str, target_lang: &str, api_key: &str) -> AppResult<TranslateResult> {
    if api_key.is_empty() {
        return Err(AppError::Translation("DeepL API key not configured".into()));
    }

    let base_url = if api_key.ends_with(":fx") {
        "https://api-free.deepl.com"
    } else {
        "https://api.deepl.com"
    };

    let tl = deepl_lang(target_lang);

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{base_url}/v2/translate"))
        .header("Authorization", format!("DeepL-Auth-Key {api_key}"))
        .form(&[("text", text), ("target_lang", tl)])
        .send()
        .await
        .map_err(|e| AppError::Translation(format!("DeepL request failed: {e}")))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(AppError::Translation(format!(
            "DeepL API error {status}: {body}"
        )));
    }

    let result: DeeplResponse = resp
        .json()
        .await
        .map_err(|e| AppError::Translation(format!("DeepL parse failed: {e}")))?;

    let first = result
        .translations
        .first()
        .ok_or_else(|| AppError::Translation("DeepL returned empty result".into()))?;

    Ok(TranslateResult {
        original: text.to_string(),
        translated: first.text.clone(),
        detected_language: first
            .detected_source_language
            .clone()
            .unwrap_or_else(|| "auto".to_string())
            .to_lowercase(),
        provider: "deepl".to_string(),
    })
}

fn deepl_lang(lang: &str) -> &str {
    match lang {
        "zh-CN" | "zh_CN" | "zh" => "ZH-HANS",
        "zh-TW" | "zh_TW" => "ZH-HANT",
        "en" => "EN",
        "ja" => "JA",
        "ko" => "KO",
        "fr" => "FR",
        "de" => "DE",
        "es" => "ES",
        "pt" => "PT-BR",
        "ru" => "RU",
        "it" => "IT",
        l => l,
    }
}
