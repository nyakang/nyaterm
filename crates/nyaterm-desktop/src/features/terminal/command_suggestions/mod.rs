//! Command completion composition; state and adapters live in focused modules.
mod diagnostics;
mod fill;
#[cfg(test)]
mod fill_tests;
mod input;
mod interaction;
pub(super) mod paste;
pub(super) mod path_source;
mod search;
pub(super) mod state;
#[cfg(test)]
mod tests;
mod view;

pub(in crate::features) use view::{
    SUGGESTION_OVERLAY_FOOTER_HEIGHT, SUGGESTION_OVERLAY_HEADER_HEIGHT,
    suggestion_overlay_desired_height,
};
