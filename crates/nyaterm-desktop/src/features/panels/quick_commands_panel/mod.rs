mod detail_card;
mod helpers;

mod panel;

// The tooltip view and its execution-mode payload are used from `panel::rows`,
// which reaches `detail_card` directly.
pub(in crate::features) use detail_card::{
    QuickCommandCardContent, QuickCommandCardExecutionMode, quick_command_detail_card,
};
