//! GPUI entity-state boundaries for the native shell.
//!
//! `NyaTermApp` and its feature-state structs are the authoritative UI state.
//! Every store here owns something the app does not: the startup-restore queue
//! and the quick switch overlay state. Read-only or otherwise unobserved
//! projections used to live here too; they were removed once it turned out
//! nothing consumed them. So was the window runtime pump, when the global
//! runtime tick it drove was replaced by the event-driven data-plane drain.

mod handles;
mod overlay;
pub(crate) mod startup_restore;

#[cfg(test)]
mod tests;

pub use handles::UiStoreHandles;
pub use overlay::{OverlayStore, QuickSwitchState};
pub use startup_restore::StartupRestoreStore;
