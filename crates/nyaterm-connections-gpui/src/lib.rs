//! Presentation components; application state and effects remain with the caller.
use nyaterm_i18n::SharedCatalogBackend;
rust_i18n::i18n!(
    "i18n-bridge",
    fallback = "en",
    backend = SharedCatalogBackend
);
pub mod details;
pub mod editor_frame;
pub mod forms;
pub mod hover_card;
pub mod models;
pub mod rows;
