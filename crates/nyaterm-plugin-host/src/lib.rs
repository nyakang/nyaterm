//! Managed plugin lifecycle, independent of GPUI and terminal transports.
pub mod authority;
pub mod catalog;
pub mod desktop_bridge;
pub mod environment;
pub mod host_io;
pub mod manager;
pub mod native;
pub mod package;
mod process;
mod process_tree;
pub mod runtime;
pub mod service;
pub mod tasks;
