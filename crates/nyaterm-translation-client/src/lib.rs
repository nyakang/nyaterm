//! Native translation provider HTTP adapters. Callers own jobs and persistence.

use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use nyaterm_core::ai::uuid;
use nyaterm_core::translation::{
    TranslateResult, TranslationError, TranslationSettings, ali_signature, ali_translate_body,
    baidu_translate_lang, baidu_translate_signature, deepl_api_base_url, deepl_translate_lang,
    format_ali_timestamp, google_translate_lang, microsoft_translate_lang,
    normalize_translation_provider, parse_ali_translate_response, parse_baidu_translate_response,
    parse_deepl_translate_response, parse_google_translate_response,
    parse_microsoft_translate_response, parse_youdao_translate_response, youdao_translate_lang,
    youdao_translate_signature,
};
use zed_reqwest::StatusCode;

const TRANSLATION_TIMEOUT: Duration = Duration::from_secs(20);

pub fn translate_text(
    provider: &str,
    text: &str,
    target_language: &str,
    settings: &TranslationSettings,
) -> Result<TranslateResult, String> {
    translate_using(
        provider,
        text,
        target_language,
        settings,
        &TranslationEndpoints::default(),
    )
}

fn translate_using(
    provider: &str,
    text: &str,
    target_language: &str,
    settings: &TranslationSettings,
    endpoints: &TranslationEndpoints,
) -> Result<TranslateResult, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err(TranslationError::EmptyText.to_string());
    }
    match normalize_translation_provider(provider).as_str() {
        "google" => translate_google(text, target_language, endpoints),
        "microsoft" => translate_microsoft(text, target_language, endpoints),
        "deepl" => translate_deepl(text, target_language, &settings.deepl_api_key, endpoints),
        "baidu" => translate_baidu(
            text,
            target_language,
            &settings.baidu_app_id,
            &settings.baidu_app_key,
            endpoints,
        ),
        "ali" => translate_ali(
            text,
            target_language,
            &settings.ali_app_id,
            &settings.ali_app_key,
            endpoints,
        ),
        "youdao" => translate_youdao(
            text,
            target_language,
            &settings.youdao_app_id,
            &settings.youdao_app_key,
            endpoints,
        ),
        other => Err(TranslationError::UnsupportedProvider(other.to_string()).to_string()),
    }
}

fn translate_google(
    text: &str,
    target_language: &str,
    endpoints: &TranslationEndpoints,
) -> Result<TranslateResult, String> {
    let target = google_translate_lang(target_language);
    let client = endpoints.client()?;
    let response = client
        .get(endpoints.url(
            "google",
            "https://translate.googleapis.com/translate_a/single",
        ))
        .query(&[
            ("client", "gtx"),
            ("sl", "auto"),
            ("tl", target),
            ("hl", target),
            ("dt", "t"),
            ("dt", "bd"),
            ("dj", "1"),
            ("source", "input"),
            ("q", text),
        ])
        .send()
        .map_err(map_translation_http_error)?;
    let status = response.status();
    let body = response.text().map_err(map_translation_http_error)?;
    if !status.is_success() {
        return Err(format!(
            "Google translate returned {}: {}",
            status_label(status),
            body.trim()
        ));
    }

    parse_google_translate_response(text, &body).map_err(|error| error.to_string())
}

fn translate_microsoft(
    text: &str,
    target_language: &str,
    endpoints: &TranslationEndpoints,
) -> Result<TranslateResult, String> {
    let client = endpoints.client()?;
    let auth = client
        .get(endpoints.url(
            "microsoft-auth",
            "https://edge.microsoft.com/translate/auth",
        ))
        .send()
        .map_err(map_translation_http_error)?;
    let auth_status = auth.status();
    let token = auth.text().map_err(map_translation_http_error)?;
    if !auth_status.is_success() {
        return Err(format!(
            "Microsoft auth returned {}: {}",
            status_label(auth_status),
            token.trim()
        ));
    }
    let response = client
        .post(endpoints.url(
            "microsoft",
            "https://api.cognitive.microsofttranslator.com/translate",
        ))
        .query(&[
            ("api-version", "3.0"),
            ("to", microsoft_translate_lang(target_language)),
        ])
        .bearer_auth(token.trim())
        .json(&serde_json::json!([{ "Text": text }]))
        .send()
        .map_err(map_translation_http_error)?;
    let status = response.status();
    let body = response.text().map_err(map_translation_http_error)?;
    if !status.is_success() {
        return Err(format!(
            "Microsoft translate returned {}: {}",
            status_label(status),
            body.trim()
        ));
    }
    parse_microsoft_translate_response(text, &body).map_err(|error| error.to_string())
}

fn translate_deepl(
    text: &str,
    target_language: &str,
    api_key: &str,
    endpoints: &TranslationEndpoints,
) -> Result<TranslateResult, String> {
    if api_key.trim().is_empty() {
        return Err(TranslationError::MissingCredentials("DeepL".to_string()).to_string());
    }
    let client = endpoints.client()?;
    let response = client
        .post(endpoints.url(
            "deepl",
            &format!("{}/v2/translate", deepl_api_base_url(api_key)),
        ))
        .header(
            "Authorization",
            format!("DeepL-Auth-Key {}", api_key.trim()),
        )
        .form(&[
            ("text", text),
            ("target_lang", deepl_translate_lang(target_language)),
        ])
        .send()
        .map_err(map_translation_http_error)?;
    let status = response.status();
    let body = response.text().map_err(map_translation_http_error)?;
    if !status.is_success() {
        return Err(format!(
            "DeepL translate returned {}: {}",
            status_label(status),
            body.trim()
        ));
    }
    parse_deepl_translate_response(text, &body).map_err(|error| error.to_string())
}

fn translate_baidu(
    text: &str,
    target_language: &str,
    app_id: &str,
    app_key: &str,
    endpoints: &TranslationEndpoints,
) -> Result<TranslateResult, String> {
    if app_id.trim().is_empty() || app_key.trim().is_empty() {
        return Err(TranslationError::MissingCredentials("Baidu".to_string()).to_string());
    }
    let salt = uuid();
    let sign = baidu_translate_signature(app_id.trim(), text, &salt, app_key.trim());
    let client = endpoints.client()?;
    let response = client
        .post(endpoints.url(
            "baidu",
            "https://fanyi-api.baidu.com/api/trans/vip/translate",
        ))
        .form(&[
            ("q", text),
            ("from", "auto"),
            ("to", baidu_translate_lang(target_language)),
            ("appid", app_id.trim()),
            ("salt", salt.as_str()),
            ("sign", sign.as_str()),
        ])
        .send()
        .map_err(map_translation_http_error)?;
    let status = response.status();
    let body = response.text().map_err(map_translation_http_error)?;
    if !status.is_success() {
        return Err(format!(
            "Baidu translate returned {}: {}",
            status_label(status),
            body.trim()
        ));
    }
    parse_baidu_translate_response(text, &body).map_err(|error| error.to_string())
}

fn translate_ali(
    text: &str,
    target_language: &str,
    app_id: &str,
    app_key: &str,
    endpoints: &TranslationEndpoints,
) -> Result<TranslateResult, String> {
    if app_id.trim().is_empty() || app_key.trim().is_empty() {
        return Err(TranslationError::MissingCredentials("Ali".to_string()).to_string());
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let date = format_ali_timestamp(now);
    let nonce = uuid();
    let signed = ali_signature(
        app_id.trim(),
        app_key.trim(),
        ali_translate_body(text, target_language),
        &date,
        &nonce,
    )
    .map_err(|error| error.to_string())?;
    let client = endpoints.client()?;
    let response = client
        .post(endpoints.url("ali", "https://mt.aliyuncs.com/"))
        .header("Authorization", signed.authorization)
        .header("x-acs-action", "TranslateGeneral")
        .header("x-acs-version", "2018-10-12")
        .header("x-acs-content-sha256", signed.content_sha256)
        .header("x-acs-date", date)
        .header("x-acs-signature-nonce", nonce)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(signed.body)
        .send()
        .map_err(map_translation_http_error)?;
    let status = response.status();
    let body = response.text().map_err(map_translation_http_error)?;
    if !status.is_success() {
        return Err(format!(
            "Ali translate returned {}: {}",
            status_label(status),
            body.trim()
        ));
    }
    parse_ali_translate_response(text, &body).map_err(|error| error.to_string())
}

fn translate_youdao(
    text: &str,
    target_language: &str,
    app_id: &str,
    app_key: &str,
    endpoints: &TranslationEndpoints,
) -> Result<TranslateResult, String> {
    if app_id.trim().is_empty() || app_key.trim().is_empty() {
        return Err(TranslationError::MissingCredentials("Youdao".to_string()).to_string());
    }
    let salt = uuid();
    let curtime = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string();
    let sign = youdao_translate_signature(app_id.trim(), text, &salt, &curtime, app_key.trim());
    let client = endpoints.client()?;
    let response = client
        .post(endpoints.url("youdao", "https://openapi.youdao.com/api"))
        .form(&[
            ("q", text),
            ("from", "auto"),
            ("to", youdao_translate_lang(target_language)),
            ("appKey", app_id.trim()),
            ("salt", salt.as_str()),
            ("sign", sign.as_str()),
            ("signType", "v3"),
            ("curtime", curtime.as_str()),
        ])
        .send()
        .map_err(map_translation_http_error)?;
    let status = response.status();
    let body = response.text().map_err(map_translation_http_error)?;
    if !status.is_success() {
        return Err(format!(
            "Youdao translate returned {}: {}",
            status_label(status),
            body.trim()
        ));
    }
    parse_youdao_translate_response(text, &body).map_err(|error| error.to_string())
}

#[derive(Default)]
struct TranslationEndpoints {
    #[cfg(test)]
    root: Option<String>,
}

impl TranslationEndpoints {
    fn url(&self, _route: &str, production: &str) -> String {
        #[cfg(test)]
        if let Some(root) = &self.root {
            return format!("{root}/{_route}");
        }
        production.to_string()
    }

    fn client(&self) -> Result<zed_reqwest::blocking::Client, String> {
        let builder = zed_reqwest::blocking::Client::builder()
            .timeout(TRANSLATION_TIMEOUT)
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36");
        #[cfg(test)]
        let builder = if self.root.is_some() {
            builder.no_proxy()
        } else {
            builder
        };
        builder.build().map_err(map_translation_http_error)
    }
}

fn map_translation_http_error(error: zed_reqwest::Error) -> String {
    if error.is_timeout() {
        format!("translation request timed out: {error}")
    } else {
        format!("translation request failed: {error}")
    }
}

fn status_label(status: StatusCode) -> String {
    status
        .canonical_reason()
        .map(|reason| format!("{status} {reason}"))
        .unwrap_or_else(|| status.to_string())
}

#[cfg(test)]
mod tests;
