//! Shared NyaTerm catalogs. No UI or application state is owned here.

rust_i18n::i18n!("locales", fallback = "en");

mod catalog;
mod locale;

// Preserve the public facade used by the desktop and presentation crates.
pub use catalog::{SharedCatalogBackend, preload};
pub use locale::{apply_locale, available_locales, locale_display_name, normalize_locale};

#[cfg(test)]
mod tests;
