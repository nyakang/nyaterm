//! Locale aliases and language-picker presentation.

use std::borrow::Cow;

use crate::catalog::CANONICAL_LOCALES;

/// The catalog id every unrecognised language falls back to. Matches the
/// `fallback` passed to `rust_i18n::i18n!` in the crate root.
const FALLBACK_LOCALE: &str = "en";

/// The single simplified-Chinese catalog. Several legacy ids map onto it.
const SIMPLIFIED_CHINESE_LOCALE: &str = "zh-CN";

/// The single traditional-Chinese catalog. Region/script variants fold here.
const TRADITIONAL_CHINESE_LOCALE: &str = "zh-TW";

/// The single Portuguese catalog. Region variants fold here.
const BRAZILIAN_PORTUGUESE_LOCALE: &str = "pt-BR";

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
/// * `pt` / `pt-*` -> `pt-BR` (only Brazilian Portuguese ships a catalog)
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
    if is_portuguese(&lower) {
        return Cow::Borrowed(BRAZILIAN_PORTUGUESE_LOCALE);
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
/// Order is fixed by the shared catalog registry rather than by
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

/// Every Portuguese tag folds onto the Brazilian catalog. European Portuguese
/// is not shipped yet; serving the Brazilian catalog is preferable to dropping
/// an existing Portuguese install back to English.
fn is_portuguese(lower: &str) -> bool {
    lower == "pt" || lower.starts_with("pt-")
}
