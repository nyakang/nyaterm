use std::borrow::Cow;

/// The catalog id every unrecognised language falls back to. Matches the
/// `fallback` passed to `rust_i18n::i18n!` in the crate root.
const FALLBACK_LOCALE: &str = "en";

/// The single simplified-Chinese catalog. Several legacy ids map onto it.
const SIMPLIFIED_CHINESE_LOCALE: &str = "zh-CN";

/// The single traditional-Chinese catalog. Region/script variants fold here.
const TRADITIONAL_CHINESE_LOCALE: &str = "zh-TW";

/// Canonical catalog ids in the order the pickers must present them.
///
/// This order is authoritative: both the settings language selector and the
/// title-bar language menu iterate [`available_locales`], so listing the ids
/// here fixes the menu order (English, 简体中文, 繁體中文, 日本語, 한국어,
/// Français) without touching the view code. Adding a language means shipping
/// its `<id>.json` catalog and adding the id here.
const CANONICAL_LOCALES: &[&str] = &["en", "zh-CN", "zh-TW", "ja", "ko", "fr"];

/// Map a stored UI language onto a catalog id rust-i18n can resolve.
///
/// rust-i18n only de-specialises by stripping subtags, so `zh-Hans` would walk to
/// `zh` and then to English because no `zh` catalog exists. NyaTerm has persisted
/// `zh`, `zh_CN`, and `zh-Hans` since the Tauri builds, and now ships several more
/// languages, so region and script aliases have to be folded onto their canonical
/// catalog here or existing installs silently switch to English.
///
/// The folding rules are:
/// * `zh-Hans` / `zh-CN` / `zh-SG` / bare `zh` -> `zh-CN`
/// * `zh-Hant` / `zh-TW` / `zh-HK` / `zh-MO` -> `zh-TW`
/// * `en-*` -> `en`, `ja-*` -> `ja`, `ko-*` -> `ko`, `fr-*` -> `fr`
/// * anything else -> `en`
pub(crate) fn normalize_locale(language: &str) -> Cow<'static, str> {
    let requested = language.trim().replace('_', "-");
    let lower = requested.to_ascii_lowercase();

    if is_simplified_chinese(&lower) {
        return Cow::Borrowed(SIMPLIFIED_CHINESE_LOCALE);
    }
    if is_traditional_chinese(&lower) {
        return Cow::Borrowed(TRADITIONAL_CHINESE_LOCALE);
    }

    // Match the exact canonical id first, then the primary language subtag, so
    // `fr-FR` and `ja-JP` resolve to their base catalog rather than English.
    let primary = lower.split('-').next().unwrap_or(&lower);
    CANONICAL_LOCALES
        .iter()
        .copied()
        .find(|canonical| {
            canonical.eq_ignore_ascii_case(&requested) || canonical.eq_ignore_ascii_case(primary)
        })
        .map(Cow::Borrowed)
        .unwrap_or(Cow::Borrowed(FALLBACK_LOCALE))
}

/// Point rust-i18n's process-wide locale at the stored UI language.
///
/// The persisted setting stays authoritative; this is a write-through projection of
/// it, so it must be called from every writer of that setting. `rust_i18n`'s locale
/// is a single global shared by every crate in the graph, which is what makes
/// `gpui-kit`'s own widget strings follow the same language.
pub(crate) fn apply_locale(language: &str) {
    rust_i18n::set_locale(&normalize_locale(language));
}

/// The locales NyaTerm ships a catalog for, in a stable, authoritative order.
///
/// Order is fixed by [`CANONICAL_LOCALES`] rather than by
/// `rust_i18n::available_locales!()`, whose ordering is unspecified. Only ids
/// that rust-i18n has actually loaded a catalog for are returned, so a canonical
/// id whose `<id>.json` is not yet shipped is skipped instead of surfacing an
/// unresolvable entry in the pickers.
pub(crate) fn available_locales() -> Vec<Cow<'static, str>> {
    let loaded = rust_i18n::available_locales!();
    CANONICAL_LOCALES
        .iter()
        .copied()
        .filter(|canonical| {
            loaded
                .iter()
                .any(|locale| locale.eq_ignore_ascii_case(canonical))
        })
        .map(Cow::Borrowed)
        .collect()
}

/// A locale's name in its own language, for the language pickers.
pub(crate) fn locale_display_name(locale: &str) -> Cow<'static, str> {
    rust_i18n::t!("language.name", locale = locale)
}

/// Language picker options, one per shipped catalog.
pub(crate) fn language_options() -> Vec<nyaterm_ui::NyaSelectOption> {
    available_locales()
        .into_iter()
        .map(|locale| {
            let label = locale_display_name(&locale);
            nyaterm_ui::NyaSelectOption::new(locale.into_owned(), label)
        })
        .collect()
}

fn is_simplified_chinese(lower: &str) -> bool {
    matches!(lower, "zh" | "zh-cn" | "zh-sg") || lower.starts_with("zh-hans")
}

fn is_traditional_chinese(lower: &str) -> bool {
    matches!(lower, "zh-tw" | "zh-hk" | "zh-mo") || lower.starts_with("zh-hant")
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashMap, HashSet};
    use std::fs;
    use std::path::Path;

    use regex::Regex;
    use serde_json::Value;

    use crate::shortcuts::{SHORTCUT_CATEGORIES, SHORTCUT_REGISTRY, ShortcutCategory};

    use super::{CANONICAL_LOCALES, Cow, available_locales, locale_display_name, normalize_locale};

    /// Translate against an explicit language instead of the process-wide locale,
    /// so no test has to touch `rust_i18n::set_locale` and race the others.
    fn text(language: &str, key: &'static str) -> Cow<'static, str> {
        let locale = normalize_locale(language);
        rust_i18n::t!(key, locale = locale.as_ref())
    }

    // The English catalog is the single schema every other catalog is measured
    // against. It is always shipped, so it always compiles in.
    const EN_JSON: &str = include_str!("../../locales/en.json");
    const ZH_CN_JSON: &str = include_str!("../../locales/zh-CN.json");

    // Every shipped catalog is included explicitly so the structural tests
    // below validate the exact bytes compiled by rust-i18n.
    const ZH_TW_JSON: &str = include_str!("../../locales/zh-TW.json");
    const JA_JSON: &str = include_str!("../../locales/ja.json");
    const KO_JSON: &str = include_str!("../../locales/ko.json");
    const FR_JSON: &str = include_str!("../../locales/fr.json");

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
        let placeholder = Regex::new(r"%\{(\w+)\}").expect("placeholder regex");
        placeholder
            .captures_iter(value)
            .map(|c| c[1].to_string())
            .collect()
    }

    #[test]
    fn resolves_tauri_locale_keys_and_normalizes_chinese_ids() {
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
    fn normalizes_region_and_script_aliases_onto_canonical_ids() {
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
    /// `quickCommands.commandPlaceholder` documents NyaTerm's own quick-command
    /// variable syntax, which is handlebars-shaped and parsed by
    /// `parse_quick_command_variables`. It is never interpolated by `t!`.
    const HANDLEBARS_IS_LITERAL: &[&str] = &["quickCommands.commandPlaceholder"];

    #[test]
    fn every_shipped_locale_names_itself_for_the_language_pickers() {
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
    fn shortcut_registry_translation_keys_exist_in_every_catalog() {
        for (locale, json) in CATALOGS {
            let catalog = flatten(json);
            for category in SHORTCUT_CATEGORIES {
                let key = category.label_key();
                let value = catalog
                    .get(key)
                    .unwrap_or_else(|| panic!("{locale} missing shortcut category key {key}"));
                assert!(!value.trim().is_empty(), "{locale}/{key} is blank");
            }
            for shortcut in SHORTCUT_REGISTRY {
                let key = shortcut.label_key;
                let value = catalog
                    .get(key)
                    .unwrap_or_else(|| panic!("{locale} missing shortcut label key {key}"));
                assert!(!value.trim().is_empty(), "{locale}/{key} is blank");
            }
        }
    }

    #[test]
    fn simplified_chinese_shortcut_labels_and_conflicts_are_localized() {
        let switch_tab = SHORTCUT_REGISTRY
            .iter()
            .find(|shortcut| shortcut.id == crate::shortcuts::ShortcutId::SwitchToTab)
            .expect("switch-tab shortcut should exist");
        let switch_tab_label = text("zh-CN", switch_tab.label_key);

        assert_eq!(
            text("zh-CN", ShortcutCategory::Terminal.label_key()),
            "终端操作"
        );
        assert_eq!(switch_tab_label, "切换到标签 1-9");
        assert_eq!(
            rust_i18n::t!(
                "settings.keybindingsConflict",
                locale = "zh-CN",
                name = switch_tab_label
            ),
            "已被占用: 切换到标签 1-9"
        );
    }

    #[test]
    fn title_bar_date_keys_are_present_and_shaped_for_every_catalog() {
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
    fn every_literal_translation_key_in_the_crate_exists_in_every_catalog() {
        // Walks the crate at test time rather than listing files, so a new module
        // cannot quietly opt out. Only literal keys are visible here - the ones
        // reached through `i18n_key()` and friends are out of reach for any
        // source scan, and stay covered by the key-parity test above.
        let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let key_pattern = Regex::new(r#"\bt!\(\s*"([^"]+)""#).expect("translation-key regex");
        let catalogs: Vec<(&str, HashMap<String, String>)> = CATALOGS
            .iter()
            .map(|(locale, json)| (*locale, flatten(json)))
            .collect();

        let mut pending = vec![source_root];
        let mut scanned = 0usize;
        let mut missing = Vec::new();
        while let Some(dir) = pending.pop() {
            for entry in fs::read_dir(&dir).expect("crate sources must be readable") {
                let path = entry.expect("directory entry").path();
                if path.is_dir() {
                    pending.push(path);
                    continue;
                }
                if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
                    continue;
                }
                let source = fs::read_to_string(&path).expect("source file must be UTF-8");
                scanned += 1;
                for captures in key_pattern.captures_iter(&source) {
                    let key = captures.get(1).expect("key capture").as_str();
                    for (locale, catalog) in &catalogs {
                        if !catalog.contains_key(key) {
                            missing.push(format!("{key} in {locale} ({})", path.display()));
                        }
                    }
                }
            }
        }

        assert!(
            scanned > 100,
            "expected to scan the whole crate, saw {scanned} files"
        );
        missing.sort();
        missing.dedup();
        assert!(
            missing.is_empty(),
            "keys missing from a catalog: {missing:#?}"
        );
    }

    #[test]
    fn connection_editor_algorithm_and_telnet_labels_are_localized() {
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
}
