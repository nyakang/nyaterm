//! Shared NyaTerm catalogs. No UI or application state is owned here.
rust_i18n::i18n!("locales", fallback = "en");
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
pub fn normalize_locale(language: &str) -> Cow<'static, str> {
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
pub fn apply_locale(language: &str) {
    rust_i18n::set_locale(&normalize_locale(language));
}

/// The locales NyaTerm ships a catalog for, in a stable, authoritative order.
///
/// Order is fixed by [`CANONICAL_LOCALES`] rather than by
/// `rust_i18n::available_locales!()`, whose ordering is unspecified. Only ids
/// that rust-i18n has actually loaded a catalog for are returned, so a canonical
/// id whose `<id>.json` is not yet shipped is skipped instead of surfacing an
/// unresolvable entry in the pickers.
pub fn available_locales() -> Vec<Cow<'static, str>> {
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
pub fn locale_display_name(locale: &str) -> Cow<'static, str> {
    rust_i18n::t!("language.name", locale = locale)
}

fn is_simplified_chinese(lower: &str) -> bool {
    matches!(lower, "zh" | "zh-cn" | "zh-sg") || lower.starts_with("zh-hans")
}

fn is_traditional_chinese(lower: &str) -> bool {
    matches!(lower, "zh-tw" | "zh-hk" | "zh-mo") || lower.starts_with("zh-hant")
}

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
                    let _ = _rust_i18n_backend();
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
        _rust_i18n_backend().available_locales()
    }
    fn translate(&self, locale: &str, key: &str) -> Option<std::borrow::Cow<'_, str>> {
        _rust_i18n_backend().translate(locale, key)
    }
    fn messages_for_locale(
        &self,
        locale: &str,
    ) -> Option<Vec<(std::borrow::Cow<'_, str>, std::borrow::Cow<'_, str>)>> {
        _rust_i18n_backend().messages_for_locale(locale)
    }
}

#[cfg(test)]
mod tests;
