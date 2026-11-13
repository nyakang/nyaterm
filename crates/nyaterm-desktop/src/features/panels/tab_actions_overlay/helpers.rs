use super::TabActionPolicy;

pub(super) const TAB_ACTIONS_MENU_WIDTH: f32 = 220.;

const MENU_ITEM_HEIGHT: f32 = 28.;
const MENU_SEPARATOR_HEIGHT: f32 = 9.;
const MENU_VERTICAL_PADDING: f32 = 8.;
const VIEWPORT_INSET: f32 = 16.;

pub(super) fn tab_actions_menu_content_height(policy: TabActionPolicy) -> f32 {
    let support = policy.support;
    let general_rows = 5 + usize::from(support.copy_ssh_host);
    let session_rows = 2 * usize::from(support.session_spawn)
        + usize::from(support.ssh_multiplex)
        + usize::from(support.reconnect)
        + usize::from(support.disconnect)
        + usize::from(support.ai)
        + usize::from(support.rdp_secure_attention);
    // Tauri always exposes Merge All Panes for terminal sessions. Flattening an
    // already-flat workspace is a harmless no-op.
    let split_rows = if support.split { 3 } else { 0 };
    let close_rows = 4 + usize::from(support.session_info);
    let row_count = general_rows + session_rows + split_rows + close_rows;
    let separator_count = policy.menu_groups().len().saturating_sub(1) + 1;

    MENU_VERTICAL_PADDING
        + row_count as f32 * MENU_ITEM_HEIGHT
        + separator_count as f32 * MENU_SEPARATOR_HEIGHT
}

pub(super) fn tab_actions_menu_visible_height(
    policy: TabActionPolicy,
    viewport_height: f32,
) -> f32 {
    let available_height = (viewport_height - VIEWPORT_INSET).max(0.);
    tab_actions_menu_content_height(policy).min(available_height)
}

pub(super) fn clamp_tab_actions_position(
    x: f32,
    y: f32,
    menu_w: f32,
    menu_h: f32,
    viewport_w: f32,
    viewport_h: f32,
) -> (f32, f32) {
    let max_x = (viewport_w - menu_w - 8.0).max(8.0);
    let max_y = (viewport_h - menu_h - 8.0).max(8.0);
    (x.clamp(8.0, max_x), y.clamp(8.0, max_y))
}

#[cfg(test)]
mod tests {
    use super::{
        TAB_ACTIONS_MENU_WIDTH, clamp_tab_actions_position, tab_actions_menu_content_height,
        tab_actions_menu_visible_height,
    };
    use crate::features::panels::tab_actions_overlay::{
        TabActionPolicy, TabActionPolicyInput, TabSessionCapability,
    };

    fn policy(session: TabSessionCapability, has_source_connection: bool) -> TabActionPolicy {
        TabActionPolicy::from_input(TabActionPolicyInput {
            session,
            has_copyable_ssh_host: session == TabSessionCapability::Ssh,
            has_source_connection,
            is_busy: false,
            is_disconnected: false,
            reconnect_pending: false,
            terminal_available: true,
            rdp_secure_attention_available: false,
            locked: false,
            tab_count: 1,
            tab_index: Some(0),
        })
    }

    #[test]
    fn main_menu_position_stays_inside_the_viewport() {
        assert_eq!(
            clamp_tab_actions_position(760., 580., TAB_ACTIONS_MENU_WIDTH, 440., 800., 600.),
            (572., 152.)
        );
    }

    #[test]
    fn content_height_tracks_protocol_specific_rows() {
        let saved_ssh = policy(TabSessionCapability::Ssh, true);
        let local = policy(TabSessionCapability::Local, false);
        let saved_vnc = policy(TabSessionCapability::Vnc, true);

        assert_eq!(tab_actions_menu_content_height(saved_ssh), 604.);
        assert_eq!(tab_actions_menu_content_height(local), 520.);
        assert_eq!(
            tab_actions_menu_content_height(policy(TabSessionCapability::AdbShell, false)),
            tab_actions_menu_content_height(local),
        );
        assert_eq!(tab_actions_menu_content_height(saved_vnc), 306.);
    }

    #[test]
    fn menu_expands_to_content_and_only_scrolls_in_short_viewports() {
        let policy = policy(TabSessionCapability::Ssh, true);

        assert_eq!(tab_actions_menu_visible_height(policy, 900.), 604.);
        assert_eq!(tab_actions_menu_visible_height(policy, 500.), 484.);
    }

    #[test]
    fn menu_clamps_all_viewport_edges_using_its_visible_height() {
        let policy = policy(TabSessionCapability::Ssh, true);
        let height = tab_actions_menu_visible_height(policy, 600.);

        assert_eq!(
            clamp_tab_actions_position(-20., -30., TAB_ACTIONS_MENU_WIDTH, height, 800., 600.),
            (8., 8.)
        );
        assert_eq!(
            clamp_tab_actions_position(900., 700., TAB_ACTIONS_MENU_WIDTH, height, 800., 600.),
            (572., 8.)
        );
    }
}
