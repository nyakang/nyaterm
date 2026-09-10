//! Native application update state and runtime.

pub(in crate::features) mod download;
mod install;
mod state;
mod update_runtime;

pub(in crate::features) use state::UpdateFeatureState;
