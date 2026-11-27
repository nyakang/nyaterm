use crate::MASKED_SECRET_VALUE;

use super::{
    TranslationError, TranslationSettings, ali_signature, ali_translate_body, ali_translate_lang,
    baidu_translate_lang, baidu_translate_signature, deepl_translate_lang, format_ali_timestamp,
    google_translate_lang, merge_masked_translation_settings, microsoft_translate_lang,
    parse_ali_translate_response, parse_baidu_translate_response, parse_deepl_translate_response,
    parse_google_translate_response, parse_microsoft_translate_response,
    parse_youdao_translate_response, youdao_translate_lang, youdao_truncate_for_sign,
};

#[test]
fn settings_debug_output_redacts_provider_keys() {
    let secret = "nya-translation-key-never-log";
    let settings = TranslationSettings {
        deepl_api_key: secret.to_string().into(),
        baidu_app_key: secret.to_string().into(),
        ali_app_key: secret.to_string().into(),
        youdao_app_key: secret.to_string().into(),
        ..TranslationSettings::default()
    };
    let output = format!("{settings:?}");

    assert!(!output.contains(secret));
    assert!(output.contains("<redacted>"));
}

#[test]
fn maps_google_language_codes_like_legacy() {
    assert_eq!(google_translate_lang("zh"), "zh-CN");
    assert_eq!(google_translate_lang("zh_TW"), "zh-TW");
    assert_eq!(google_translate_lang("ja"), "ja");
    assert_eq!(google_translate_lang(""), "zh-CN");
    assert_eq!(microsoft_translate_lang("zh-CN"), "zh-Hans");
    assert_eq!(deepl_translate_lang("pt"), "PT-BR");
    assert_eq!(baidu_translate_lang("ja"), "jp");
    assert_eq!(ali_translate_lang("zh-TW"), "zh-tw");
    assert_eq!(youdao_translate_lang("zh"), "zh-CHS");
}

#[test]
fn maps_brazilian_portuguese_language_codes() {
    // The translation picker stores the legacy "pt" identifier.
    for language in ["pt", "pt-BR", "pt_BR"] {
        assert_eq!(google_translate_lang(language), "pt");
        assert_eq!(microsoft_translate_lang(language), "pt-BR");
        assert_eq!(deepl_translate_lang(language), "PT-BR");
        assert_eq!(baidu_translate_lang(language), "pt");
        assert_eq!(ali_translate_lang(language), "pt");
        assert_eq!(youdao_translate_lang(language), "pt");
    }
}

#[test]
fn parses_google_translate_response() {
    let body = r#"{
        "sentences": [
            {"trans":"你好"},
            {"trans":"，世界"}
        ],
        "src": "en"
    }"#;
    let result = parse_google_translate_response("hello world", body).expect("parse");

    assert_eq!(result.original, "hello world");
    assert_eq!(result.translated, "你好，世界");
    assert_eq!(result.detected_language, "en");
    assert_eq!(result.provider, "google");
    assert_eq!(
        parse_google_translate_response("x", r#"{"sentences":[]}"#).unwrap_err(),
        TranslationError::EmptyResult("Google".to_string())
    );
}

#[test]
fn parses_commercial_provider_responses() {
    let microsoft = parse_microsoft_translate_response(
        "hello",
        r#"[{"detectedLanguage":{"language":"en"},"translations":[{"text":"你好","to":"zh-Hans"}]}]"#,
    )
    .expect("microsoft");
    assert_eq!(microsoft.translated, "你好");
    assert_eq!(microsoft.detected_language, "en");
    assert_eq!(microsoft.provider, "microsoft");

    let deepl = parse_deepl_translate_response(
        "hello",
        r#"{"translations":[{"text":"Bonjour","detected_source_language":"EN"}]}"#,
    )
    .expect("deepl");
    assert_eq!(deepl.translated, "Bonjour");
    assert_eq!(deepl.detected_language, "en");

    let baidu = parse_baidu_translate_response(
        "hello\nworld",
        r#"{"from":"en","to":"zh","trans_result":[{"dst":"你好"},{"dst":"世界"}]}"#,
    )
    .expect("baidu");
    assert_eq!(baidu.translated, "你好\n世界");

    let youdao = parse_youdao_translate_response(
        "hello",
        r#"{"errorCode":"0","translation":["你好"],"l":"en2zh-CHS"}"#,
    )
    .expect("youdao");
    assert_eq!(youdao.detected_language, "en");

    let ali = parse_ali_translate_response(
        "hello",
        r#"{"Code":"200","Data":{"Translated":"你好","DetectedLanguage":"en"}}"#,
    )
    .expect("ali");
    assert_eq!(ali.provider, "ali");
}

#[test]
fn builds_legacy_provider_signatures() {
    assert_eq!(
        baidu_translate_signature("app", "hello", "salt", "key"),
        "6f41caee5f563445e6713d84080f3f33"
    );
    assert_eq!(
        youdao_truncate_for_sign("abcdefghijklmnopqrstuvwxyz"),
        "abcdefghij26qrstuvwxyz"
    );
    assert_eq!(
        youdao_truncate_for_sign("一二三四五六七八九十十一二三四五六七八九十十一"),
        "一二三四五六七八九十23三四五六七八九十十一"
    );
    assert_eq!(format_ali_timestamp(0), "1970-01-01T00:00:00Z");
    let body = ali_translate_body("hello world", "zh-CN");
    assert_eq!(
        body,
        "FormatType=text&SourceLanguage=auto&TargetLanguage=zh&SourceText=hello%20world&Scene=general"
    );
    let signature = ali_signature("app", "key", body.clone(), "1970-01-01T00:00:00Z", "nonce")
        .expect("ali signature");
    assert_eq!(signature.body, body);
    assert_eq!(
        signature.content_sha256,
        "68933ac8caba2e99ae36a59ffeeee356453a105246560a79b864cb28a98de908"
    );
    assert!(signature.authorization.starts_with("ACS3-HMAC-SHA256 "));
}

#[test]
fn merges_masked_translation_secrets() {
    let current = TranslationSettings {
        target_language: "ja".to_string(),
        deepl_api_key: "deepl-secret".to_string().into(),
        baidu_app_id: "baidu-id".to_string(),
        baidu_app_key: "baidu-secret".to_string().into(),
        ..TranslationSettings::default()
    };
    let next = TranslationSettings {
        deepl_api_key: MASKED_SECRET_VALUE.to_string().into(),
        baidu_app_key: String::new().into(),
        ..current.clone()
    };
    let merged = merge_masked_translation_settings(&current, next);
    assert_eq!(merged.deepl_api_key, "deepl-secret");
    assert_eq!(merged.baidu_app_key, "");
}
