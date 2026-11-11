//! UI-independent terminal interaction policies shared by desktop adapters.
//!
//! The child modules are grouped here by terminal concern. The crate root keeps
//! compatibility modules with their historical names and re-exports the same
//! public symbols for existing consumers.

pub mod connection_input;
pub mod dynamic_title;
pub mod editing;
pub mod file_drop;
pub mod input_fanout;
pub mod input_tracker;
pub mod mouse;
pub mod resize;
pub mod shell_prompt;
pub mod windows_terminal;
pub mod wire_write;
