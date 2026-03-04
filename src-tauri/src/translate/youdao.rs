use crate::error::{AppError, AppResult};
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

use super::TranslateResult;

#[derive(serde::Deserialize)]
struct YoudaoResponse {
    #[serde(rename = "errorCode")]
    error_code: String,
    translation: Option<Vec<String>>,
    l: Option<String>,
}

fn truncate_for_sign(q: &str) -> String {
    let len = q.len();
    if len <= 20 {
        q.to_string()
    } else {
        format!("{}{len}{}", &q[..10], &q[len - 10..])
    }
}

pub async fn translate(
    text: &str,
    target_lang: &str,
    app_id: &str,
    app_key: &str,
) -> AppResult<TranslateResult> {
    if app_id.is_empty() || app_key.is_empty() {
        return Err(AppError::Translation(
            "Youdao App ID and App Key not configured".into(),
        ));
    }

    let salt = uuid::Uuid::new_v4().to_string();
    let curtime = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .to_string();

    let q_in_sign = truncate_for_sign(text);
    let sign_str = format!("{app_id}{q_in_sign}{salt}{curtime}{app_key}");
    let mut hasher = Sha256::new();
    hasher.update(sign_str.as_bytes());
    let sign = hex::encode(hasher.finalize());

    let tl = youdao_lang(target_lang);

    let client = reqwest::Client::new();
    let resp = client
        .post("https://openapi.youdao.com/api")
        .form(&[
            ("q", text),
            ("from", "auto"),
            ("to", tl),
            ("appKey", app_id),
            ("salt", &salt),
            ("sign", &sign),
            ("signType", "v3"),
            ("curtime", &curtime),
        ])
        .send()
        .await
        .map_err(|e| AppError::Translation(format!("Youdao request failed: {e}")))?;

    let result: YoudaoResponse = resp
        .json()
        .await
        .map_err(|e| AppError::Translation(format!("Youdao parse failed: {e}")))?;

    if result.error_code != "0" {
        return Err(AppError::Translation(format!(
            "Youdao error code: {}",
            result.error_code
        )));
    }

    let translated = result
        .translation
        .as_ref()
        .and_then(|t| t.first())
        .cloned()
        .unwrap_or_default();

    if translated.is_empty() {
        return Err(AppError::Translation("Youdao returned empty result".into()));
    }

    let detected = result
        .l
        .as_ref()
        .and_then(|l| l.split('2').next())
        .unwrap_or("auto")
        .to_string();

    Ok(TranslateResult {
        original: text.to_string(),
        translated,
        detected_language: detected,
        provider: "youdao".to_string(),
    })
}

fn youdao_lang(lang: &str) -> &str {
    match lang {
        "zh-CN" | "zh_CN" | "zh" => "zh-CHS",
        "zh-TW" | "zh_TW" => "zh-CHT",
        "en" => "en",
        "ja" => "ja",
        "ko" => "ko",
        "fr" => "fr",
        "de" => "de",
        "es" => "es",
        "pt" => "pt",
        "ru" => "ru",
        "it" => "it",
        l => l,
    }
}
