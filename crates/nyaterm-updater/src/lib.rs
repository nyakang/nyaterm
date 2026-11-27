//! UI-independent native update checks, signed downloads, and installation.
//! Application state, scheduling, and progress-window presentation belong to the caller.

pub mod check;
pub mod download;
pub mod install;

pub mod error;

#[cfg(target_os = "linux")]
pub mod linux;
