//! Compatibility facade for UI-independent command/connection policy.
pub(super) use nyaterm_core::quick_command_script::{
    QuickCommandChunk, plan_literal_quick_command, plan_quick_command, quick_command_needs_staging,
};
