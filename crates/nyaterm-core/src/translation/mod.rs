use hmac::{Hmac, Mac, digest::KeyInit as HmacKeyInit};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{MASKED_SECRET_VALUE, SecretString};

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TranslateResult {
    pub original: String,
    pub translated: String,
    pub detected_language: String,
    pub provider: String,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TranslationSettings {
    #[serde(default = "default_translation_target_language")]
    pub target_language: String,
    #[serde(default)]
    pub deepl_api_key: SecretString,
    #[serde(default)]
    pub baidu_app_id: String,
    #[serde(default)]
    pub baidu_app_key: SecretString,
    #[serde(default)]
    pub ali_app_id: String,
    #[serde(default)]
    pub ali_app_key: SecretString,
    #[serde(default)]
    pub youdao_app_id: String,
    #[serde(default)]
    pub youdao_app_key: SecretString,
}

impl Default for TranslationSettings {
    fn default() -> Self {
        Self {
            target_language: default_translation_target_language(),
            deepl_api_key: SecretString::default(),
            baidu_app_id: String::new(),
            baidu_app_key: SecretString::default(),
            ali_app_id: String::new(),
            ali_app_key: SecretString::default(),
            youdao_app_id: String::new(),
            youdao_app_key: SecretString::default(),
        }
    }
}

impl std::fmt::Debug for TranslationSettings {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TranslationSettings")
            .field("target_language", &self.target_language)
            .field("deepl_api_key", &"<redacted>")
            .field("baidu_app_id", &self.baidu_app_id)
            .field("baidu_app_key", &"<redacted>")
            .field("ali_app_id", &self.ali_app_id)
            .field("ali_app_key", &"<redacted>")
            .field("youdao_app_id", &self.youdao_app_id)
            .field("youdao_app_key", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum TranslationError {
    #[error("translation text is empty")]
    EmptyText,
    #[error("{0} credentials are not configured")]
    MissingCredentials(String),
    #[error("unsupported translation provider: {0}")]
    UnsupportedProvider(String),
    #[error("invalid translation JSON: {0}")]
    InvalidJson(String),
    #[error("{0} returned empty translation")]
    EmptyResult(String),
    #[error("{0} error: {1}")]
    ProviderError(String, String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AliSignature {
    pub body: String,
    pub content_sha256: String,
    pub authorization: String,
}

fn default_translation_target_language() -> String {
    "zh-CN".to_string()
}

pub fn normalize_translation_provider(provider: &str) -> String {
    let provider = provider.trim().to_ascii_lowercase();
    if provider.is_empty() {
        "google".to_string()
    } else {
        provider
    }
}

pub fn google_translate_lang(lang: &str) -> &str {
    match lang.trim() {
        "zh-CN" | "zh_CN" | "zh" => "zh-CN",
        "zh-TW" | "zh_TW" => "zh-TW",
        "pt-BR" | "pt_BR" => "pt",
        "" => "zh-CN",
        value => value,
    }
}

pub fn microsoft_translate_lang(lang: &str) -> &str {
    match lang.trim() {
        "zh-CN" | "zh_CN" | "zh" => "zh-Hans",
        "zh-TW" | "zh_TW" => "zh-Hant",
        "pt-BR" | "pt_BR" => "pt-BR",
        "" => "zh-Hans",
        value => value,
    }
}

pub fn deepl_translate_lang(lang: &str) -> &str {
    match lang.trim() {
        "zh-CN" | "zh_CN" | "zh" => "ZH-HANS",
        "zh-TW" | "zh_TW" => "ZH-HANT",
        "en" => "EN",
        "ja" => "JA",
        "ko" => "KO",
        "fr" => "FR",
        "de" => "DE",
        "es" => "ES",
        "pt" | "pt-BR" | "pt_BR" => "PT-BR",
        "ru" => "RU",
        "it" => "IT",
        "" => "ZH-HANS",
        value => value,
    }
}

pub fn deepl_api_base_url(api_key: &str) -> &'static str {
    if api_key.ends_with(":fx") {
        "https://api-free.deepl.com"
    } else {
        "https://api.deepl.com"
    }
}

pub fn baidu_translate_lang(lang: &str) -> &str {
    match lang.trim() {
        "zh-CN" | "zh_CN" | "zh" => "zh",
        "zh-TW" | "zh_TW" => "cht",
        "en" => "en",
        "ja" => "jp",
        "ko" => "kor",
        "fr" => "fra",
        "de" => "de",
        "es" => "spa",
        "pt" | "pt-BR" | "pt_BR" => "pt",
        "ru" => "ru",
        "it" => "it",
        "" => "zh",
        value => value,
    }
}

pub fn ali_translate_lang(lang: &str) -> &str {
    match lang.trim() {
        "zh-CN" | "zh_CN" | "zh" => "zh",
        "zh-TW" | "zh_TW" => "zh-tw",
        "en" => "en",
        "ja" => "ja",
        "ko" => "ko",
        "fr" => "fr",
        "de" => "de",
        "es" => "es",
        "pt" | "pt-BR" | "pt_BR" => "pt",
        "ru" => "ru",
        "it" => "it",
        "" => "zh",
        value => value,
    }
}

pub fn youdao_translate_lang(lang: &str) -> &str {
    match lang.trim() {
        "zh-CN" | "zh_CN" | "zh" => "zh-CHS",
        "zh-TW" | "zh_TW" => "zh-CHT",
        "en" => "en",
        "ja" => "ja",
        "ko" => "ko",
        "fr" => "fr",
        "de" => "de",
        "es" => "es",
        "pt" | "pt-BR" | "pt_BR" => "pt",
        "ru" => "ru",
        "it" => "it",
        "" => "zh-CHS",
        value => value,
    }
}

pub fn parse_google_translate_response(
    original: &str,
    body: &str,
) -> Result<TranslateResult, TranslationError> {
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|error| TranslationError::InvalidJson(error.to_string()))?;
    let mut translated = String::new();
    if let Some(sentences) = value.get("sentences").and_then(serde_json::Value::as_array) {
        for sentence in sentences {
            if let Some(text) = sentence.get("trans").and_then(serde_json::Value::as_str) {
                translated.push_str(text);
            }
        }
    }

    if translated.trim().is_empty() {
        return Err(TranslationError::EmptyResult("Google".to_string()));
    }

    let detected_language = value
        .get("src")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("auto")
        .to_string();

    Ok(TranslateResult {
        original: original.to_string(),
        translated,
        detected_language,
        provider: "google".to_string(),
    })
}

pub fn parse_microsoft_translate_response(
    original: &str,
    body: &str,
) -> Result<TranslateResult, TranslationError> {
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|error| TranslationError::InvalidJson(error.to_string()))?;
    let first = value
        .as_array()
        .and_then(|items| items.first())
        .ok_or_else(|| TranslationError::EmptyResult("Microsoft".to_string()))?;
    let translated = first
        .get("translations")
        .and_then(serde_json::Value::as_array)
        .and_then(|items| items.first())
        .and_then(|item| item.get("text"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    if translated.trim().is_empty() {
        return Err(TranslationError::EmptyResult("Microsoft".to_string()));
    }
    let detected_language = first
        .get("detectedLanguage")
        .and_then(|item| item.get("language"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("auto")
        .to_string();
    Ok(TranslateResult {
        original: original.to_string(),
        translated,
        detected_language,
        provider: "microsoft".to_string(),
    })
}

pub fn parse_deepl_translate_response(
    original: &str,
    body: &str,
) -> Result<TranslateResult, TranslationError> {
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|error| TranslationError::InvalidJson(error.to_string()))?;
    let first = value
        .get("translations")
        .and_then(serde_json::Value::as_array)
        .and_then(|items| items.first())
        .ok_or_else(|| TranslationError::EmptyResult("DeepL".to_string()))?;
    let translated = first
        .get("text")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    if translated.trim().is_empty() {
        return Err(TranslationError::EmptyResult("DeepL".to_string()));
    }
    let detected_language = first
        .get("detected_source_language")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("auto")
        .to_ascii_lowercase();
    Ok(TranslateResult {
        original: original.to_string(),
        translated,
        detected_language,
        provider: "deepl".to_string(),
    })
}

pub fn baidu_translate_signature(app_id: &str, text: &str, salt: &str, app_key: &str) -> String {
    format!(
        "{:x}",
        md5::compute(format!("{app_id}{text}{salt}{app_key}").as_bytes())
    )
}

pub fn parse_baidu_translate_response(
    original: &str,
    body: &str,
) -> Result<TranslateResult, TranslationError> {
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|error| TranslationError::InvalidJson(error.to_string()))?;
    if let Some(code) = value.get("error_code").and_then(serde_json::Value::as_str) {
        let message = value
            .get("error_msg")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("Unknown error");
        return Err(TranslationError::ProviderError(
            "Baidu".to_string(),
            format!("{code}: {message}"),
        ));
    }
    let items = value
        .get("trans_result")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| TranslationError::EmptyResult("Baidu".to_string()))?;
    let translated = items
        .iter()
        .filter_map(|item| item.get("dst").and_then(serde_json::Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    if translated.trim().is_empty() {
        return Err(TranslationError::EmptyResult("Baidu".to_string()));
    }
    let detected_language = value
        .get("from")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("auto")
        .to_string();
    Ok(TranslateResult {
        original: original.to_string(),
        translated,
        detected_language,
        provider: "baidu".to_string(),
    })
}

pub fn youdao_truncate_for_sign(text: &str) -> String {
    let len = text.chars().count();
    if len <= 20 {
        return text.to_string();
    }
    let head = text.chars().take(10).collect::<String>();
    let tail = text
        .chars()
        .rev()
        .take(10)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("{head}{len}{tail}")
}

pub fn youdao_translate_signature(
    app_id: &str,
    text: &str,
    salt: &str,
    curtime: &str,
    app_key: &str,
) -> String {
    let input = youdao_truncate_for_sign(text);
    let mut hasher = Sha256::new();
    hasher.update(format!("{app_id}{input}{salt}{curtime}{app_key}").as_bytes());
    hex::encode(hasher.finalize())
}

pub fn parse_youdao_translate_response(
    original: &str,
    body: &str,
) -> Result<TranslateResult, TranslationError> {
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|error| TranslationError::InvalidJson(error.to_string()))?;
    let error_code = value
        .get("errorCode")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("0");
    if error_code != "0" {
        return Err(TranslationError::ProviderError(
            "Youdao".to_string(),
            format!("code {error_code}"),
        ));
    }
    let translated = value
        .get("translation")
        .and_then(serde_json::Value::as_array)
        .and_then(|items| items.first())
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    if translated.trim().is_empty() {
        return Err(TranslationError::EmptyResult("Youdao".to_string()));
    }
    let detected_language = value
        .get("l")
        .and_then(serde_json::Value::as_str)
        .and_then(|value| value.split('2').next())
        .unwrap_or("auto")
        .to_string();
    Ok(TranslateResult {
        original: original.to_string(),
        translated,
        detected_language,
        provider: "youdao".to_string(),
    })
}

pub fn ali_translate_body(text: &str, target_language: &str) -> String {
    format!(
        "FormatType=text&SourceLanguage=auto&TargetLanguage={}&SourceText={}&Scene=general",
        percent_encode_form_component(ali_translate_lang(target_language)),
        percent_encode_form_component(text)
    )
}

pub fn ali_content_sha256(body: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(body.as_bytes());
    hex::encode(hasher.finalize())
}

pub fn format_ali_timestamp(secs: u64) -> String {
    let days_since_epoch = secs / 86_400;
    let time_of_day = secs % 86_400;
    let hours = time_of_day / 3_600;
    let minutes = (time_of_day % 3_600) / 60;
    let seconds = time_of_day % 60;
    let (year, month, day) = days_to_ymd(days_since_epoch);
    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}Z")
}

pub fn ali_signature(
    app_id: &str,
    app_key: &str,
    body: String,
    date: &str,
    nonce: &str,
) -> Result<AliSignature, TranslationError> {
    let content_sha256 = ali_content_sha256(&body);
    let headers_to_sign = format!(
        "host:mt.aliyuncs.com\nx-acs-action:TranslateGeneral\nx-acs-content-sha256:{content_sha256}\nx-acs-date:{date}\nx-acs-signature-nonce:{nonce}\nx-acs-version:2018-10-12"
    );
    let signed_headers =
        "host;x-acs-action;x-acs-content-sha256;x-acs-date;x-acs-signature-nonce;x-acs-version";
    let canonical_request =
        format!("POST\n/\n\n{headers_to_sign}\n\n{signed_headers}\n{content_sha256}");
    let mut request_hasher = Sha256::new();
    request_hasher.update(canonical_request.as_bytes());
    let hashed_request = hex::encode(request_hasher.finalize());
    let string_to_sign = format!("ACS3-HMAC-SHA256\n{hashed_request}");
    let mut mac = HmacSha256::new_from_slice(app_key.as_bytes())
        .map_err(|error| TranslationError::ProviderError("Ali".to_string(), error.to_string()))?;
    mac.update(string_to_sign.as_bytes());
    let signature = hex::encode(mac.finalize().into_bytes());
    let authorization = format!(
        "ACS3-HMAC-SHA256 Credential={app_id},SignedHeaders={signed_headers},Signature={signature}"
    );
    Ok(AliSignature {
        body,
        content_sha256,
        authorization,
    })
}

pub fn parse_ali_translate_response(
    original: &str,
    body: &str,
) -> Result<TranslateResult, TranslationError> {
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|error| TranslationError::InvalidJson(error.to_string()))?;
    if let Some(code) = value.get("Code").and_then(serde_json::Value::as_str)
        && code != "200"
    {
        let message = value
            .get("Message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("Unknown error");
        return Err(TranslationError::ProviderError(
            "Ali".to_string(),
            format!("{code}: {message}"),
        ));
    }
    let data = value
        .get("Data")
        .ok_or_else(|| TranslationError::EmptyResult("Ali".to_string()))?;
    let translated = data
        .get("Translated")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    if translated.trim().is_empty() {
        return Err(TranslationError::EmptyResult("Ali".to_string()));
    }
    let detected_language = data
        .get("DetectedLanguage")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("auto")
        .to_string();
    Ok(TranslateResult {
        original: original.to_string(),
        translated,
        detected_language,
        provider: "ali".to_string(),
    })
}

pub fn translation_settings_has_secret(settings: &TranslationSettings) -> bool {
    [
        &settings.deepl_api_key,
        &settings.baidu_app_key,
        &settings.ali_app_key,
        &settings.youdao_app_key,
    ]
    .iter()
    .any(|value| {
        let value = value.expose_secret().trim();
        !value.is_empty() && value != MASKED_SECRET_VALUE
    })
}

pub fn merge_masked_translation_settings(
    current: &TranslationSettings,
    mut next: TranslationSettings,
) -> TranslationSettings {
    next.deepl_api_key = merge_masked_secret(&current.deepl_api_key, next.deepl_api_key);
    next.baidu_app_key = merge_masked_secret(&current.baidu_app_key, next.baidu_app_key);
    next.ali_app_key = merge_masked_secret(&current.ali_app_key, next.ali_app_key);
    next.youdao_app_key = merge_masked_secret(&current.youdao_app_key, next.youdao_app_key);
    next
}

fn merge_masked_secret(current: &SecretString, next: SecretString) -> SecretString {
    if next.expose_secret().trim() == MASKED_SECRET_VALUE {
        current.clone()
    } else {
        next
    }
}

fn percent_encode_form_component(input: &str) -> String {
    let mut output = String::new();
    for byte in input.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                output.push(char::from(*byte));
            }
            _ => output.push_str(&format!("%{byte:02X}")),
        }
    }
    output
}

fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    (year, month, day)
}

#[cfg(test)]
mod tests;
