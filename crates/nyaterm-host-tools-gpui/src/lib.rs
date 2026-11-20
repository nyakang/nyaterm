//! Ports and Services panels with caller-supplied host, appearance and confirmation.
mod panel;

pub use panel::confirmation::{HostToolConfirmation, HostToolConfirmationHandler};
#[cfg(any(test, feature = "test-support"))]
pub use panel::test_support;
pub use panel::{HostContext, HostResolution, HostToolKind, HostToolsAppearance, HostToolsPanel};

use nyaterm_i18n::SharedCatalogBackend;
rust_i18n::i18n!(
    "i18n-bridge",
    fallback = "en",
    backend = SharedCatalogBackend
);
