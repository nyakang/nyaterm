//! Remote monitor views. The caller owns data, host identity, jobs and authorization.
pub mod actions;
pub mod docker;
mod docker_view;
pub mod formatting;
pub mod list_window;
pub mod models;
pub mod network;
pub mod panels;
pub mod presentation;
mod process;
mod process_view;
mod stats_view;
#[cfg(test)]
mod test_support;
use nyaterm_i18n::SharedCatalogBackend;
rust_i18n::i18n!(
    "i18n-bridge",
    fallback = "en",
    backend = SharedCatalogBackend
);
