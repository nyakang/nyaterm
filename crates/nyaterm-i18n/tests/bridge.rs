use nyaterm_i18n::{SharedCatalogBackend, apply_locale, preload};
rust_i18n::i18n!(
    "tests/empty",
    fallback = "en",
    backend = SharedCatalogBackend
);

#[test]
fn shared_backend_preserves_caller_macros_fallback_interpolation_and_global_locale() {
    preload().unwrap();
    preload().unwrap();
    let previous = rust_i18n::locale().to_string();
    apply_locale("zh_Hans");
    assert_eq!(rust_i18n::t!("language.name"), "中文 (简体)");
    assert_eq!(rust_i18n::t!("language.name", locale = "ja"), "日本語");
    assert_eq!(
        rust_i18n::t!("language.name", locale = "unsupported"),
        "English"
    );
    let key = "language.name";
    assert_eq!(rust_i18n::t!(key, locale = "fr"), "Français");
    assert_eq!(rust_i18n::available_locales!().len(), 6);
    assert_eq!(
        rust_i18n::t!(
            "updater.releaseDateShort",
            locale = "en",
            year = "2026",
            month = "11",
            day = "15"
        ),
        "2026-11-15"
    );
    rust_i18n::set_locale(&previous);
}
