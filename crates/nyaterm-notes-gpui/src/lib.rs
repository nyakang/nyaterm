//! Presentation components; application state and effects remain with the caller.
use nyaterm_i18n::SharedCatalogBackend;
rust_i18n::i18n!(
    "i18n-bridge",
    fallback = "en",
    backend = SharedCatalogBackend
);
pub mod editor;
pub mod menus;
pub mod tree;
