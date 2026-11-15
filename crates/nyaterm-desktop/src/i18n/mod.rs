//! GPUI adapters for the shared locale catalog.
pub(crate) use nyaterm_i18n::{
    apply_locale, available_locales, locale_display_name, normalize_locale,
};
pub(crate) fn configure_input_labels(cx: &mut gpui::App) {
    nyaterm_ui::set_nya_input_label_provider(
        || {
            nyaterm_ui::NyaInputLabels::new(
                rust_i18n::t!("input.showSecret"),
                rust_i18n::t!("input.hideSecret"),
            )
        },
        cx,
    );
}
pub(crate) fn language_options() -> Vec<nyaterm_ui::NyaSelectOption> {
    available_locales()
        .into_iter()
        .map(|locale| {
            let label = locale_display_name(&locale);
            nyaterm_ui::NyaSelectOption::new(locale.into_owned(), label)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::shortcuts::{SHORTCUT_CATEGORIES, SHORTCUT_REGISTRY, ShortcutCategory};
    use regex::Regex;
    use rust_i18n::Backend as _;
    use std::borrow::Cow;
    use std::collections::HashMap;
    use std::fs;
    use std::path::Path;
    fn text(language: &str, key: &'static str) -> Cow<'static, str> {
        crate::preload_i18n().unwrap();
        rust_i18n::t!(
            key,
            locale = nyaterm_i18n::normalize_locale(language).as_ref()
        )
    }
    fn catalogs() -> Vec<(String, HashMap<String, String>)> {
        crate::preload_i18n().unwrap();
        nyaterm_i18n::available_locales()
            .into_iter()
            .map(|locale| {
                let entries = nyaterm_i18n::SharedCatalogBackend
                    .messages_for_locale(&locale)
                    .unwrap()
                    .into_iter()
                    .map(|(key, value)| (key.into_owned(), value.into_owned()))
                    .collect();
                (locale.into_owned(), entries)
            })
            .collect()
    }
    #[test]
    fn shortcut_registry_translation_keys_exist_in_every_catalog() {
        for (locale, catalog) in catalogs() {
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
    fn every_literal_translation_key_in_the_crate_exists_in_every_catalog() {
        // Walks the crate at test time rather than listing files, so a new module
        // cannot quietly opt out. Only literal keys are visible here - the ones
        // reached through `i18n_key()` and friends are out of reach for any
        // source scan, and stay covered by the key-parity test above.
        let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let key_pattern = Regex::new(r#"\bt!\(\s*"([^"]+)""#).expect("translation-key regex");
        let catalogs = catalogs();

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
}
