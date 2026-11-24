//! Presentation components; application state and effects remain with the caller.
use nyaterm_i18n::SharedCatalogBackend;
rust_i18n::i18n!(
    "i18n-bridge",
    fallback = "en",
    backend = SharedCatalogBackend
);
pub mod completion;
pub mod editor;
pub mod formatting;
pub mod path_navigation;
pub mod preview;
pub mod preview_controls;
pub mod queue;
pub mod table;
