//! Native application update state and runtime.

pub(in crate::features) mod download;
mod failure;
pub(crate) mod install;
mod state;
mod update_runtime;

pub(in crate::features) use failure::{UpdateFailure, UpdateFailureKind};

pub(crate) use state::{UpdateCheckKind, UpdateEvent, UpdatePhase, UpdateStore};

#[cfg(target_os = "linux")]
mod linux;
