//! Presentation components; application state and effects remain with the caller.
use nyaterm_i18n::SharedCatalogBackend;
rust_i18n::i18n!(
    "i18n-bridge",
    fallback = "en",
    backend = SharedCatalogBackend
);
pub mod approvals;
pub mod components;
pub mod composer;
pub mod layout;
pub mod messages;
pub mod presentation;
pub mod tool_call;
