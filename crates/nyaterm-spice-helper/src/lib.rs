//! Native SPICE decoder boundary. This crate must never be linked by the app.

mod audio;
pub mod driver;
pub mod frame;
pub mod native;
#[cfg(target_os = "linux")]
pub mod pipe_capacity;
mod relay;
