use std::collections::{BTreeSet, HashMap, HashSet};

use regex::Regex;
use serde_json::Value;

use super::{CANONICAL_LOCALES, Cow, available_locales, locale_display_name, normalize_locale};

/// Translate against an explicit language instead of the process-wide locale,
/// so no test has to touch `rust_i18n::set_locale` and race the others.
fn text(language: &str, key: &'static str) -> Cow<'static, str> {
    let locale = normalize_locale(language);
    rust_i18n::t!(key, locale = locale.as_ref())
}

// The English catalog is the single schema every other catalog is measured
// against. It is always shipped, so it always compiles in.
const EN_JSON: &str = include_str!("../locales/en.json");
const ZH_CN_JSON: &str = include_str!("../locales/zh-CN.json");

// Every shipped catalog is included explicitly so the structural tests
// below validate the exact bytes compiled by rust-i18n.
const ZH_TW_JSON: &str = include_str!("../locales/zh-TW.json");
const JA_JSON: &str = include_str!("../locales/ja.json");
const KO_JSON: &str = include_str!("../locales/ko.json");
const FR_JSON: &str = include_str!("../locales/fr.json");

/// Every catalog currently shipped, paired with its canonical id. The
/// structural tests below iterate this table, so extending it is the only
/// change a new language's catalog needs to be fully validated.
const CATALOGS: &[(&str, &str)] = &[
    ("en", EN_JSON),
    ("zh-CN", ZH_CN_JSON),
    ("zh-TW", ZH_TW_JSON),
    ("ja", JA_JSON),
    ("ko", KO_JSON),
    ("fr", FR_JSON),
];

/// Mirror of the flattening `rust_i18n` applies to a version-1 locale file, so
/// these tests can reason about the on-disk files rather than the loaded catalog.
fn flatten(json: &str) -> HashMap<String, String> {
    fn walk(prefix: Option<&str>, value: &Value, output: &mut HashMap<String, String>) {
        let Value::Object(entries) = value else {
            return;
        };
        for (name, value) in entries {
            let key = prefix.map_or_else(|| name.clone(), |prefix| format!("{prefix}.{name}"));
            match value {
                Value::String(text) => {
                    output.insert(key, text.clone());
                }
                Value::Object(_) => walk(Some(&key), value, output),
                _ => {}
            }
        }
    }

    let value: Value = serde_json::from_str(json).expect("locale JSON must be valid");
    let mut output = HashMap::new();
    walk(None, &value, &mut output);
    output
}

/// The set of `%{name}` placeholders a translation references.
fn placeholder_names(value: &str) -> BTreeSet<String> {
    static PLACEHOLDER: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| Regex::new(r"%\{(\w+)\}").expect("placeholder regex"));
    let placeholder = &*PLACEHOLDER;
    placeholder
        .captures_iter(value)
        .map(|c| c[1].to_string())
        .collect()
}

#[test]
fn every_android_label_and_error_is_translated_in_all_six_catalogs() {
    crate::preload().unwrap();
    let english = flatten(EN_JSON);
    let keys: Vec<_> = english
        .keys()
        .filter(|key| key.starts_with("android."))
        .collect();
    assert!(keys.len() >= 60);
    for (locale, json) in CATALOGS {
        let catalog = flatten(json);
        for key in &keys {
            let value = catalog
                .get(*key)
                .unwrap_or_else(|| panic!("{locale}: missing {key}"));
            assert!(!value.trim().is_empty(), "{locale}: {key}");
            assert_eq!(
                placeholder_names(value),
                placeholder_names(&english[*key]),
                "{locale}: {key}"
            );
            assert_eq!(
                rust_i18n::t!(key.as_str(), locale = *locale),
                value.as_str(),
                "{locale}: {key}"
            );
        }
    }
}

#[test]
fn resolves_tauri_locale_keys_and_normalizes_chinese_ids() {
    crate::preload().unwrap();
    assert_eq!(text("en", "menu.file"), "File");
    assert_eq!(text("zh-CN", "menu.file"), "文件");
    assert_eq!(text("zh_CN", "common.cancel"), "取消");
    assert_eq!(text("zh-Hans", "settings.title"), "设置");
    assert_eq!(text("en", "common.copyToClipboard"), "Copy");
    assert_eq!(text("zh-CN", "common.copyToClipboard"), "复制");
    assert_eq!(text("en", "common.retry"), "Retry");
    assert_eq!(text("zh-CN", "common.retry"), "重试");
}

#[test]
fn falls_back_to_english_then_the_key() {
    crate::preload().unwrap();
    // `fr` ships a catalog now, so it resolves to its own translation.
    assert_eq!(text("fr", "menu.help"), "Aide");
    // `de` has no shipped catalog, so it resolves back to English.
    assert_eq!(text("de", "menu.help"), "Help");
    assert_eq!(
        text("zh-CN", "missing.translation.key"),
        "missing.translation.key"
    );
}

#[test]
fn every_shipped_locale_translates_secret_input_actions() {
    crate::preload().unwrap();
    for (locale, json) in CATALOGS {
        let catalog = flatten(json);
        for key in ["input.showSecret", "input.hideSecret"] {
            let label = catalog
                .get(key)
                .expect("secret input action must be translated");
            assert!(!label.trim().is_empty(), "{locale}: {key}");
            assert_eq!(text(locale, key), label.as_str(), "{locale}: {key}");
        }
        assert_ne!(catalog["input.showSecret"], catalog["input.hideSecret"]);
    }
}

#[test]
fn normalizes_region_and_script_aliases_onto_canonical_ids() {
    crate::preload().unwrap();
    for simplified in [
        "zh",
        "zh-CN",
        "zh_CN",
        "zh-Hans",
        "zh-hans-cn",
        "zh-SG",
        " zh-cn ",
    ] {
        assert_eq!(normalize_locale(simplified), "zh-CN", "for {simplified:?}");
    }
    for traditional in ["zh-TW", "zh_TW", "zh-Hant", "zh-hant-tw", "zh-HK", "zh-MO"] {
        assert_eq!(
            normalize_locale(traditional),
            "zh-TW",
            "for {traditional:?}"
        );
    }
    assert_eq!(normalize_locale("ja"), "ja");
    assert_eq!(normalize_locale("ja-JP"), "ja");
    assert_eq!(normalize_locale("ko"), "ko");
    assert_eq!(normalize_locale("ko-KR"), "ko");
    assert_eq!(normalize_locale("fr"), "fr");
    assert_eq!(normalize_locale("fr-FR"), "fr");
    assert_eq!(normalize_locale("en"), "en");
    assert_eq!(normalize_locale("en-US"), "en");
    for unknown in ["", "nonsense", "de", "es-ES"] {
        assert_eq!(normalize_locale(unknown), "en", "for {unknown:?}");
    }
}

#[test]
fn every_canonical_locale_normalizes_to_itself() {
    crate::preload().unwrap();
    // A canonical id must be a fixed point of normalization, or storing the
    // value the picker offers would silently rewrite it to something else.
    for canonical in CANONICAL_LOCALES {
        assert_eq!(
            normalize_locale(canonical),
            *canonical,
            "canonical id {canonical:?} did not normalize to itself"
        );
    }
}

#[test]
fn available_locales_follow_the_canonical_menu_order() {
    crate::preload().unwrap();
    // The pickers must expose every shipped catalog exactly once in the
    // product-defined order, rather than rust-i18n's unspecified order.
    let available = available_locales();
    assert_eq!(
        available, CANONICAL_LOCALES,
        "language picker catalogs changed"
    );

    let loaded = rust_i18n::available_locales!()
        .into_iter()
        .map(|locale| locale.into_owned())
        .collect::<HashSet<_>>();
    let canonical = CANONICAL_LOCALES
        .iter()
        .map(|locale| (*locale).to_string())
        .collect::<HashSet<_>>();
    assert_eq!(
        loaded, canonical,
        "rust-i18n loaded an unexpected catalog set"
    );
}

/// Entries whose `{{..}}` is literal text rather than an i18n slot.
///
/// The quick-command placeholders document NyaTerm's own variable syntax, which
/// is handlebars-shaped and parsed by `parse_quick_command_variables`. They are
/// never interpolated by `t!`.
const HANDLEBARS_IS_LITERAL: &[&str] = &[
    "quickCommands.commandPlaceholder",
    "quickCommands.segmentedCommandPlaceholder",
];

#[test]
fn every_shipped_locale_names_itself_for_the_language_pickers() {
    crate::preload().unwrap();
    // The selector and the title-bar menu label each entry with
    // `language.name` read in that entry's own locale, so a catalog without
    // one would show a bare key.
    for (locale, json) in CATALOGS {
        let flattened = flatten(json);
        let name = flattened
            .get("language.name")
            .unwrap_or_else(|| panic!("{locale} has no language.name"));
        assert!(
            !name.trim().is_empty(),
            "{locale} has a blank language.name"
        );
        // The loaded catalog must agree with the on-disk file.
        assert_eq!(
            locale_display_name(locale),
            name.as_str(),
            "{locale} language.name disagrees between file and catalog"
        );
    }
    assert_eq!(locale_display_name("en"), "English");
    assert_eq!(locale_display_name("zh-CN"), "中文 (简体)");
    assert_eq!(locale_display_name("zh-TW"), "繁體中文");
    assert_eq!(locale_display_name("ja"), "日本語");
    assert_eq!(locale_display_name("ko"), "한국어");
    assert_eq!(locale_display_name("fr"), "Français");
}

#[test]
fn no_catalog_leaves_handlebars_placeholders_behind() {
    crate::preload().unwrap();
    // A leftover `{{name}}` would render literally: rust-i18n only substitutes
    // `%{name}`, and nothing else interpolates these strings any more.
    let mut stragglers = Vec::new();
    for (locale, json) in CATALOGS {
        for (key, value) in flatten(json) {
            if value.contains("{{") && !HANDLEBARS_IS_LITERAL.contains(&key.as_str()) {
                stragglers.push(format!("{locale}/{key}: {value}"));
            }
        }
    }
    stragglers.sort();
    assert!(
        stragglers.is_empty(),
        "handlebars placeholders left behind: {stragglers:#?}"
    );
}

#[test]
fn no_catalog_has_blank_translations() {
    crate::preload().unwrap();
    let mut blanks = Vec::new();
    for (locale, json) in CATALOGS {
        for (key, value) in flatten(json) {
            if value.trim().is_empty() {
                blanks.push(format!("{locale}/{key}"));
            }
        }
    }
    blanks.sort();
    assert!(blanks.is_empty(), "blank translations: {blanks:#?}");
}

#[test]
fn every_catalog_agrees_with_english_on_placeholders() {
    crate::preload().unwrap();
    // A translation that drops or renames a placeholder leaves the argument
    // unsubstituted for that locale only, which no other test would catch.
    let english = flatten(EN_JSON);
    let mut mismatched = Vec::new();
    for (locale, json) in CATALOGS {
        if *locale == "en" {
            continue;
        }
        let catalog = flatten(json);
        for (key, en_value) in &english {
            if HANDLEBARS_IS_LITERAL.contains(&key.as_str()) {
                continue;
            }
            let Some(value) = catalog.get(key) else {
                continue; // covered by the key-parity test
            };
            let (want, got) = (placeholder_names(en_value), placeholder_names(value));
            if want != got {
                mismatched.push(format!("{locale}/{key}: en {want:?} vs {got:?}"));
            }
        }
    }
    mismatched.sort();
    assert!(
        mismatched.is_empty(),
        "placeholder mismatch: {mismatched:#?}"
    );
}

#[test]
fn every_catalog_has_the_same_keys_as_english() {
    crate::preload().unwrap();
    let english = flatten(EN_JSON).into_keys().collect::<HashSet<_>>();
    for (locale, json) in CATALOGS {
        if *locale == "en" {
            continue;
        }
        let catalog = flatten(json).into_keys().collect::<HashSet<_>>();
        let missing = english.difference(&catalog).cloned().collect::<Vec<_>>();
        let extra = catalog.difference(&english).cloned().collect::<Vec<_>>();
        assert!(
            missing.is_empty() && extra.is_empty(),
            "{locale} key mismatch — missing {missing:?}, extra {extra:?}"
        );
    }
}

#[test]
fn title_bar_date_keys_are_present_and_shaped_for_every_catalog() {
    crate::preload().unwrap();
    // The header clock feeds `date`, `time`, and `weekday` into
    // `titleBar.dateTime`, and looks weekday short names up per locale, so
    // every catalog must carry all seven weekday keys and a template that
    // references the three slots.
    let weekday_keys = [
        "titleBar.weekday.monday",
        "titleBar.weekday.tuesday",
        "titleBar.weekday.wednesday",
        "titleBar.weekday.thursday",
        "titleBar.weekday.friday",
        "titleBar.weekday.saturday",
        "titleBar.weekday.sunday",
    ];
    for (locale, json) in CATALOGS {
        let flattened = flatten(json);
        for key in weekday_keys {
            let value = flattened
                .get(key)
                .unwrap_or_else(|| panic!("{locale} missing {key}"));
            assert!(!value.trim().is_empty(), "{locale}/{key} is blank");
        }
        let template = flattened
            .get("titleBar.dateTime")
            .unwrap_or_else(|| panic!("{locale} missing titleBar.dateTime"));
        let slots = placeholder_names(template);
        for slot in ["date", "time", "weekday"] {
            assert!(
                slots.contains(slot),
                "{locale} titleBar.dateTime is missing %{{{slot}}}: {template:?}"
            );
        }
    }
}

#[test]
fn title_bar_date_renders_through_the_catalog() {
    crate::preload().unwrap();
    assert_eq!(text("en", "titleBar.weekday.monday"), "Mon");
    assert_eq!(text("zh-CN", "titleBar.weekday.monday"), "周一");
    assert_eq!(
        rust_i18n::t!(
            "titleBar.dateTime",
            locale = "en",
            date = "2026-07-27",
            time = "09:05",
            weekday = "Mon"
        ),
        "Mon, 2026-07-27 09:05"
    );
    assert_eq!(
        rust_i18n::t!(
            "titleBar.dateTime",
            locale = "zh-CN",
            date = "2026-07-27",
            time = "09:05",
            weekday = "周一"
        ),
        "2026-07-27 09:05 周一"
    );
}

#[test]
fn connection_editor_algorithm_and_telnet_labels_are_localized() {
    crate::preload().unwrap();
    assert_eq!(text("en", "dialog.sshAlgorithms"), "SSH algorithms");
    assert_eq!(text("zh-CN", "dialog.sshAlgorithms"), "SSH 算法");
    assert_eq!(text("en", "dialog.telnetAutoLogin"), "Auto Login");
    assert_eq!(text("zh-CN", "dialog.telnetAutoLogin"), "自动登录");
    assert_eq!(
        rust_i18n::t!(
            "dialog.algorithmUnsupportedError",
            locale = "en",
            algorithm = "future-kex",
            category = "key exchanges"
        ),
        "future-kex is not supported in key exchanges."
    );
}
