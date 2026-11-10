//! Compatibility facade for UI-independent command/connection policy.
pub(crate) use nyaterm_core::action_links::{
    ActionLinkAction, ActionLinkKind, ActionLinkMatch, ActionLinkPlatform, ActionLinkTarget,
    match_at_offset,
};

/// Plan actions for a known execution platform and localize their labels.
pub(crate) fn actions_for_match_on(
    item: &ActionLinkMatch,
    platform: ActionLinkPlatform,
) -> Vec<ActionLinkAction> {
    let mut actions = nyaterm_core::action_links::actions_for_match_on(item, platform);
    if item.kind == ActionLinkKind::Mac {
        for action in &mut actions {
            action.label = match action.target {
                ActionLinkTarget::CopyText(_) => rust_i18n::t!("terminal.macCopy").to_string(),
                ActionLinkTarget::PasteText(_) => rust_i18n::t!("terminal.macInsert").to_string(),
                _ => action.label.clone(),
            };
        }
    }
    actions
}
