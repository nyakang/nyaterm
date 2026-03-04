use crate::error::{AppError, AppResult};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

use super::TranslateResult;

type HmacSha256 = Hmac<Sha256>;

#[derive(serde::Deserialize)]
struct AliResponse {
    #[serde(rename = "Code")]
    code: Option<String>,
    #[serde(rename = "Message")]
    message: Option<String>,
    #[serde(rename = "Data")]
    data: Option<AliData>,
}

#[derive(serde::Deserialize)]
struct AliData {
    #[serde(rename = "Translated")]
    translated: String,
    #[serde(rename = "DetectedLanguage")]
    detected_language: Option<String>,
}

pub async fn translate(
    text: &str,
    target_lang: &str,
    app_id: &str,
    app_key: &str,
) -> AppResult<TranslateResult> {
    if app_id.is_empty() || app_key.is_empty() {
        return Err(AppError::Translation(
            "Ali App ID and App Key not configured".into(),
        ));
    }

    let tl = ali_lang(target_lang);
    let body = format!(
        "FormatType=text&SourceLanguage=auto&TargetLanguage={}&SourceText={}&Scene=general",
        urlencoding::encode(tl),
        urlencoding::encode(text)
    );

    let nonce = uuid::Uuid::new_v4().to_string();

    let mut body_hasher = Sha256::new();
    body_hasher.update(body.as_bytes());
    let body_hash = hex::encode(body_hasher.finalize());

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let date = format_iso8601(now);

    let headers_to_sign = format!(
        "host:mt.aliyuncs.com\nx-acs-action:TranslateGeneral\nx-acs-content-sha256:{body_hash}\nx-acs-date:{date}\nx-acs-signature-nonce:{nonce}\nx-acs-version:2018-10-12"
    );
    let signed_headers =
        "host;x-acs-action;x-acs-content-sha256;x-acs-date;x-acs-signature-nonce;x-acs-version";

    let canonical_request = format!(
        "POST\n/\n\n{headers_to_sign}\n\n{signed_headers}\n{body_hash}"
    );

    let mut req_hasher = Sha256::new();
    req_hasher.update(canonical_request.as_bytes());
    let hashed_request = hex::encode(req_hasher.finalize());

    let string_to_sign = format!("ACS3-HMAC-SHA256\n{hashed_request}");

    let mut mac = HmacSha256::new_from_slice(app_key.as_bytes())
        .map_err(|e| AppError::Translation(format!("HMAC init failed: {e}")))?;
    mac.update(string_to_sign.as_bytes());
    let signature = hex::encode(mac.finalize().into_bytes());

    let authorization = format!(
        "ACS3-HMAC-SHA256 Credential={app_id},SignedHeaders={signed_headers},Signature={signature}"
    );

    let client = reqwest::Client::new();
    let resp = client
        .post("https://mt.aliyuncs.com/")
        .header("Authorization", &authorization)
        .header("x-acs-action", "TranslateGeneral")
        .header("x-acs-version", "2018-10-12")
        .header("x-acs-content-sha256", &body_hash)
        .header("x-acs-date", &date)
        .header("x-acs-signature-nonce", &nonce)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .map_err(|e| AppError::Translation(format!("Ali request failed: {e}")))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(AppError::Translation(format!(
            "Ali API error {status}: {body}"
        )));
    }

    let result: AliResponse = resp
        .json()
        .await
        .map_err(|e| AppError::Translation(format!("Ali parse failed: {e}")))?;

    if let Some(code) = &result.code {
        if code != "200" {
            let msg = result.message.as_deref().unwrap_or("Unknown error");
            return Err(AppError::Translation(format!("Ali error {code}: {msg}")));
        }
    }

    let data = result
        .data
        .ok_or_else(|| AppError::Translation("Ali returned empty result".into()))?;

    Ok(TranslateResult {
        original: text.to_string(),
        translated: data.translated,
        detected_language: data.detected_language.unwrap_or_else(|| "auto".to_string()),
        provider: "ali".to_string(),
    })
}

fn ali_lang(lang: &str) -> &str {
    match lang {
        "zh-CN" | "zh_CN" | "zh" => "zh",
        "zh-TW" | "zh_TW" => "zh-tw",
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

fn format_iso8601(secs: u64) -> String {
    let days_since_epoch = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    let (year, month, day) = days_to_ymd(days_since_epoch);
    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}Z")
}

fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}
