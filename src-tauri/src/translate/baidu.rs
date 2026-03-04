use crate::error::{AppError, AppResult};
use md5::{Digest, Md5};
use serde::Deserialize;

use super::TranslateResult;

#[derive(Deserialize)]
struct BaiduResponse {
    from: Option<String>,
    trans_result: Option<Vec<BaiduTrans>>,
    error_code: Option<String>,
    error_msg: Option<String>,
}

#[derive(Deserialize)]
struct BaiduTrans {
    dst: String,
}

pub async fn translate(
    text: &str,
    target_lang: &str,
    app_id: &str,
    app_key: &str,
) -> AppResult<TranslateResult> {
    if app_id.is_empty() || app_key.is_empty() {
        return Err(AppError::Translation(
            "Baidu App ID and App Key not configured".into(),
        ));
    }

    let salt = uuid::Uuid::new_v4().to_string();
    let sign_str = format!("{app_id}{text}{salt}{app_key}");
    let mut hasher = Md5::new();
    hasher.update(sign_str.as_bytes());
    let sign = hex::encode(hasher.finalize());

    let tl = baidu_lang(target_lang);

    let client = reqwest::Client::new();
    let resp = client
        .post("https://fanyi-api.baidu.com/api/trans/vip/translate")
        .form(&[
            ("q", text),
            ("from", "auto"),
            ("to", tl),
            ("appid", app_id),
            ("salt", &salt),
            ("sign", &sign),
        ])
        .send()
        .await
        .map_err(|e| AppError::Translation(format!("Baidu request failed: {e}")))?;

    let result: BaiduResponse = resp
        .json()
        .await
        .map_err(|e| AppError::Translation(format!("Baidu parse failed: {e}")))?;

    if let Some(code) = &result.error_code {
        let msg = result.error_msg.as_deref().unwrap_or("Unknown error");
        return Err(AppError::Translation(format!(
            "Baidu error {code}: {msg}"
        )));
    }

    let trans = result
        .trans_result
        .as_ref()
        .and_then(|t| t.first())
        .ok_or_else(|| AppError::Translation("Baidu returned empty result".into()))?;

    let all_translated: String = result
        .trans_result
        .as_ref()
        .map(|items| {
            items
                .iter()
                .map(|t| t.dst.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_else(|| trans.dst.clone());

    Ok(TranslateResult {
        original: text.to_string(),
        translated: all_translated,
        detected_language: result.from.unwrap_or_else(|| "auto".to_string()),
        provider: "baidu".to_string(),
    })
}

fn baidu_lang(lang: &str) -> &str {
    match lang {
        "zh-CN" | "zh_CN" | "zh" => "zh",
        "zh-TW" | "zh_TW" => "cht",
        "en" => "en",
        "ja" => "jp",
        "ko" => "kor",
        "fr" => "fra",
        "de" => "de",
        "es" => "spa",
        "pt" => "pt",
        "ru" => "ru",
        "it" => "it",
        l => l,
    }
}
