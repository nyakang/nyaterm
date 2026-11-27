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
    use rust_i18n::Backend as _;
    use std::borrow::Cow;
    use std::collections::HashMap;
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
}
