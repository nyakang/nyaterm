//! The single compiled catalog, its consumer bridge and startup initialization.

// One registry controls picker order, incremental rebuild tracking and fixtures.
// Production tracks source bytes without retaining a second runtime catalog.
macro_rules! catalogs {
    ($($locale:literal),+ $(,)?) => {
        pub(crate) const CANONICAL_LOCALES: &[&str] = &[$($locale),+];

        // rust-i18n's macro reads files outside rustc's dependency tracking.
        const _: &[&str] = &[$(include_str!(concat!("../locales/", $locale, ".json"))),+];

        #[cfg(test)]
        pub(crate) const CATALOGS: &[(&str, &str)] = &[
            $(($locale, include_str!(concat!("../locales/", $locale, ".json")))),+
        ];
    };
}

catalogs!("en", "zh-CN", "zh-TW", "ja", "ko", "fr", "pt-BR");

const I18N_PRELOAD_STACK_BYTES: usize = 16 * 1024 * 1024;
static I18N_PRELOAD_RESULT: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();

/// Build rust-i18n's generated backend before the first UI render.
///
/// rust-i18n 4.2 expands every catalog entry into one lazy initializer. With
/// NyaTerm's full multilingual catalog that initializer exceeds the default
/// Windows main-thread stack. A short-lived worker gives that one-time build a
/// bounded stack without moving GPUI off the platform main thread or increasing
/// the executable's stack reservation permanently.
pub fn preload() -> Result<(), String> {
    I18N_PRELOAD_RESULT
        .get_or_init(|| {
            std::thread::Builder::new()
                .name("nyaterm-i18n-preload".to_string())
                .stack_size(I18N_PRELOAD_STACK_BYTES)
                .spawn(|| {
                    let _ = crate::_rust_i18n_backend();
                })
                .map_err(|error| format!("failed to spawn i18n preload worker: {error}"))?
                .join()
                .map_err(|_| "i18n preload worker panicked".to_string())
        })
        .clone()
}

/// A lightweight adapter for rust-i18n's public `backend` option.
/// It borrows the single generated catalog; consumers compile no copy of it.
#[derive(Clone, Copy, Default)]
pub struct SharedCatalogBackend;
impl rust_i18n::Backend for SharedCatalogBackend {
    fn available_locales(&self) -> Vec<std::borrow::Cow<'_, str>> {
        crate::_rust_i18n_backend().available_locales()
    }
    fn translate(&self, locale: &str, key: &str) -> Option<std::borrow::Cow<'_, str>> {
        crate::_rust_i18n_backend().translate(locale, key)
    }
    fn messages_for_locale(
        &self,
        locale: &str,
    ) -> Option<Vec<(std::borrow::Cow<'_, str>, std::borrow::Cow<'_, str>)>> {
        crate::_rust_i18n_backend().messages_for_locale(locale)
    }
}
