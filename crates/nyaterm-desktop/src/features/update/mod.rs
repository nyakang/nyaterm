//! Native application update state and runtime.

pub(in crate::features) mod download;
mod failure;
pub(crate) mod install;
mod state;
mod update_runtime;

pub(in crate::features) use failure::{UpdateFailure, UpdateFailureKind, classify_update_failure};

pub(crate) use state::{UpdateCheckKind, UpdateEvent, UpdatePhase, UpdateStore};
