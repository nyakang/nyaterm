use crate::error::{AppError, AppResult};
use serde::Deserialize;

use super::TranslateResult;

#[derive(Deserialize)]
struct MsTranslation {
    #[serde(default)]
    translations: Vec<MsTranslationItem>,
    #[serde(rename = "detectedLanguage")]
    detected_language: Option<MsDetectedLang>,
}

#[derive(Deserialize)]
struct MsTranslationItem {
    text: String,
    #[allow(dead_code)]
    to: String,
}

#[derive(Deserialize)]
struct MsDetectedLang {
    language: String,
}

async fn get_auth_token() -> AppResult<String> {
    let client = reqwest::Client::new();
    let resp = client
        .get("https://edge.microsoft.com/translate/auth")
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .send()
        .await
        .map_err(|e| AppError::Translation(format!("Microsoft auth failed: {e}")))?;

    resp.text()
        .await
        .map_err(|e| AppError::Translation(format!("Microsoft auth read failed: {e}")))
}

pub async fn translate(text: &str, target_lang: &str) -> AppResult<TranslateResult> {
    let token = get_auth_token().await?;
    let client = reqwest::Client::new();

    let tl = ms_lang(target_lang);

    let body = serde_json::json!([{ "Text": text }]);

    let resp = client
        .post("https://api.cognitive.microsofttranslator.com/translate")
        .query(&[("api-version", "3.0"), ("to", tl)])
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| AppError::Translation(format!("Microsoft request failed: {e}")))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(AppError::Translation(format!(
            "Microsoft API error {status}: {body}"
        )));
    }

    let results: Vec<MsTranslation> = resp
        .json()
        .await
        .map_err(|e| AppError::Translation(format!("Microsoft parse failed: {e}")))?;

    let first = results
        .first()
        .ok_or_else(|| AppError::Translation("Microsoft returned empty result".into()))?;

    let translation = first
        .translations
        .first()
        .ok_or_else(|| AppError::Translation("Microsoft returned no translations".into()))?;

    let detected = first
        .detected_language
        .as_ref()
        .map(|d| d.language.clone())
        .unwrap_or_else(|| "auto".to_string());

    Ok(TranslateResult {
        original: text.to_string(),
        translated: translation.text.clone(),
        detected_language: detected,
        provider: "microsoft".to_string(),
    })
}

fn ms_lang(lang: &str) -> &str {
    match lang {
        "zh-CN" | "zh_CN" => "zh-Hans",
        "zh-TW" | "zh_TW" => "zh-Hant",
        l => l,
    }
}
