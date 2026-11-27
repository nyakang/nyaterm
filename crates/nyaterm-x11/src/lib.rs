//! Blocking platform adapter. Callers schedule preparation off the UI thread.
mod managed;
mod process;
mod system;

pub use managed::start_managed;
pub use system::{prepare_external, probe};
