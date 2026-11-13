use crate::models::{SessionLaunchConfig, TabActionsSubmenu};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::features) enum TabSessionCapability {
    Local,
    Ssh,
    Telnet,
    Serial,
    Rdp,
    Vnc,
    AdbShell,
    AdbView,
}

impl TabSessionCapability {
    pub(in crate::features) fn from_launch_config(config: &SessionLaunchConfig) -> Self {
        match config {
            SessionLaunchConfig::Local(_) => Self::Local,
            SessionLaunchConfig::Ssh(_) => Self::Ssh,
            SessionLaunchConfig::Telnet(_) => Self::Telnet,
            SessionLaunchConfig::Serial(_) => Self::Serial,
            SessionLaunchConfig::Rdp(_) => Self::Rdp,
            SessionLaunchConfig::Vnc(_) => Self::Vnc,
            SessionLaunchConfig::AdbShell(_) => Self::AdbShell,
            SessionLaunchConfig::AdbFiles(_) | SessionLaunchConfig::AdbScreen(_) => Self::AdbView,
        }
    }

    pub(in crate::features) fn supports_terminal_actions(self) -> bool {
        matches!(
            self,
            Self::Local | Self::Ssh | Self::Telnet | Self::Serial | Self::AdbShell
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::features) struct TabActionSupport {
    pub copy_ssh_host: bool,
    pub session_spawn: bool,
    pub ssh_multiplex: bool,
    pub reconnect: bool,
    pub disconnect: bool,
    pub ai: bool,
    pub rdp_secure_attention: bool,
    pub android_screen: bool,
    pub split: bool,
    pub session_info: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::features) struct TabActionAvailability {
    pub spawn_session: bool,
    pub multiplex: bool,
    pub reconnect: bool,
    pub disconnect: bool,
    pub use_ai: bool,
    pub rdp_secure_attention: bool,
    pub android_screen: bool,
    pub split: bool,
    pub close_tab: bool,
    pub close_inactive: bool,
    pub close_right: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::features) struct TabActionPolicy {
    pub support: TabActionSupport,
    pub availability: TabActionAvailability,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::features) struct TabActionPolicyInput {
    pub session: TabSessionCapability,
    pub has_copyable_ssh_host: bool,
    pub has_source_connection: bool,
    pub is_busy: bool,
    pub is_disconnected: bool,
    pub reconnect_pending: bool,
    pub terminal_available: bool,
    pub rdp_secure_attention_available: bool,
    pub locked: bool,
    pub tab_count: usize,
    pub tab_index: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::features) enum TabActionMenuGroup {
    General,
    Session,
    Split,
    Close,
}

impl TabActionPolicy {
    pub(in crate::features) fn from_input(input: TabActionPolicyInput) -> Self {
        let supports_terminal_actions = input.session.supports_terminal_actions();
        let support = TabActionSupport {
            copy_ssh_host: input.session == TabSessionCapability::Ssh
                && input.has_copyable_ssh_host,
            session_spawn: supports_terminal_actions,
            ssh_multiplex: input.session == TabSessionCapability::Ssh,
            reconnect: supports_terminal_actions || input.session == TabSessionCapability::AdbView,
            disconnect: supports_terminal_actions || input.session == TabSessionCapability::AdbView,
            ai: supports_terminal_actions,
            rdp_secure_attention: input.session == TabSessionCapability::Rdp,
            android_screen: matches!(
                input.session,
                TabSessionCapability::AdbShell | TabSessionCapability::AdbView
            ),
            split: supports_terminal_actions || input.session == TabSessionCapability::AdbView,
            session_info: input.has_source_connection,
        };
        let availability = TabActionAvailability {
            // GPUI duplicate/split can recreate a disconnected terminal from its retained
            // launch config. Their execution paths do not use the per-session busy guard.
            spawn_session: support.session_spawn,
            multiplex: support.ssh_multiplex && !input.is_busy && !input.is_disconnected,
            // Tauri allows reconnecting both live and disconnected sessions. A live
            // reconnect intentionally closes and recreates the backend in place.
            reconnect: support.reconnect && !input.is_busy && !input.reconnect_pending,
            disconnect: support.disconnect && !input.is_busy && !input.is_disconnected,
            use_ai: support.ai
                && input.terminal_available
                && !input.is_busy
                && !input.is_disconnected,
            rdp_secure_attention: support.rdp_secure_attention
                && input.rdp_secure_attention_available,
            android_screen: support.android_screen
                && !input.is_busy
                && !input.is_disconnected
                && !input.reconnect_pending,
            split: support.split,
            close_tab: !input.locked,
            close_inactive: input.tab_count > 1,
            close_right: input
                .tab_index
                .is_some_and(|index| index + 1 < input.tab_count),
        };
        Self {
            support,
            availability,
        }
    }

    pub(in crate::features) fn menu_groups(self) -> Vec<TabActionMenuGroup> {
        let mut groups = vec![TabActionMenuGroup::General];
        if self.shows_session_group() {
            groups.push(TabActionMenuGroup::Session);
        }
        if self.support.split {
            groups.push(TabActionMenuGroup::Split);
        }
        groups.push(TabActionMenuGroup::Close);
        groups
    }

    pub(in crate::features) fn shows_session_group(self) -> bool {
        self.support.session_spawn
            || self.support.ssh_multiplex
            || self.support.reconnect
            || self.support.disconnect
            || self.support.ai
            || self.support.rdp_secure_attention
    }

    pub(in crate::features) fn supports_submenu(self, submenu: TabActionsSubmenu) -> bool {
        match submenu {
            TabActionsSubmenu::Color => true,
            TabActionsSubmenu::MoveWindow => true,
            TabActionsSubmenu::SshAdvanced => self.support.ssh_multiplex,
            TabActionsSubmenu::Ai => self.support.ai,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{TabActionMenuGroup, TabActionPolicy, TabActionPolicyInput, TabSessionCapability};

    fn input(session: TabSessionCapability) -> TabActionPolicyInput {
        TabActionPolicyInput {
            session,
            has_copyable_ssh_host: false,
            has_source_connection: false,
            is_busy: false,
            is_disconnected: false,
            reconnect_pending: false,
            terminal_available: true,
            rdp_secure_attention_available: false,
            locked: false,
            tab_count: 1,
            tab_index: Some(0),
        }
    }

    #[test]
    fn local_terminal_supports_terminal_actions_without_protocol_specific_items() {
        let policy = TabActionPolicy::from_input(input(TabSessionCapability::Local));

        assert!(policy.support.session_spawn);
        assert!(policy.support.reconnect);
        assert!(policy.support.disconnect);
        assert!(policy.support.ai);
        assert!(policy.support.split);
        assert!(!policy.support.copy_ssh_host);
        assert!(!policy.support.ssh_multiplex);
        assert!(!policy.support.session_info);
        assert_eq!(
            policy.menu_groups(),
            vec![
                TabActionMenuGroup::General,
                TabActionMenuGroup::Session,
                TabActionMenuGroup::Split,
                TabActionMenuGroup::Close,
            ]
        );
    }

    #[test]
    fn ssh_host_and_source_connection_control_visibility_independently() {
        let mut context = input(TabSessionCapability::Ssh);
        let without_metadata = TabActionPolicy::from_input(context);
        assert!(!without_metadata.support.copy_ssh_host);
        assert!(!without_metadata.support.session_info);
        // GPUI temporary SSH sessions retain their full launch config even without a source id.
        assert!(without_metadata.support.session_spawn);
        assert!(without_metadata.support.reconnect);
        assert!(without_metadata.support.split);
        assert!(without_metadata.support.ssh_multiplex);

        context.has_copyable_ssh_host = true;
        context.has_source_connection = true;
        let with_metadata = TabActionPolicy::from_input(context);
        assert!(with_metadata.support.copy_ssh_host);
        assert!(with_metadata.support.session_info);
        assert!(with_metadata.availability.multiplex);
    }

    #[test]
    fn telnet_and_serial_are_terminal_capable_but_never_show_ssh_actions() {
        for session in [TabSessionCapability::Telnet, TabSessionCapability::Serial] {
            let policy = TabActionPolicy::from_input(input(session));
            assert!(policy.support.session_spawn);
            assert!(policy.support.disconnect);
            assert!(policy.support.ai);
            assert!(policy.support.split);
            assert!(!policy.support.copy_ssh_host);
            assert!(!policy.support.ssh_multiplex);
        }
    }

    #[test]
    fn rdp_shows_secure_attention_with_runtime_capability_controlling_availability() {
        let mut context = input(TabSessionCapability::Rdp);
        context.has_source_connection = true;
        let unavailable = TabActionPolicy::from_input(context);

        assert!(!unavailable.support.session_spawn);
        assert!(!unavailable.support.ssh_multiplex);
        assert!(!unavailable.support.reconnect);
        assert!(!unavailable.support.disconnect);
        assert!(!unavailable.support.ai);
        assert!(!unavailable.support.split);
        assert!(unavailable.support.rdp_secure_attention);
        assert!(!unavailable.availability.rdp_secure_attention);
        assert!(unavailable.support.session_info);
        assert_eq!(
            unavailable.menu_groups(),
            vec![
                TabActionMenuGroup::General,
                TabActionMenuGroup::Session,
                TabActionMenuGroup::Close,
            ]
        );

        context.rdp_secure_attention_available = true;
        let available = TabActionPolicy::from_input(context);
        assert!(available.availability.rdp_secure_attention);
    }

    #[test]
    fn vnc_omits_terminal_and_rdp_session_actions() {
        let mut context = input(TabSessionCapability::Vnc);
        context.has_source_connection = true;
        let policy = TabActionPolicy::from_input(context);

        assert!(!policy.support.session_spawn);
        assert!(!policy.support.rdp_secure_attention);
        assert!(policy.support.session_info);
        assert_eq!(
            policy.menu_groups(),
            vec![TabActionMenuGroup::General, TabActionMenuGroup::Close]
        );
    }

    #[test]
    fn live_and_disconnected_terminals_allow_reconnect_when_idle() {
        let live = TabActionPolicy::from_input(input(TabSessionCapability::Ssh));
        assert!(live.support.reconnect);
        assert!(live.availability.reconnect);

        let mut context = input(TabSessionCapability::Ssh);
        context.is_disconnected = true;
        let policy = TabActionPolicy::from_input(context);

        assert!(policy.support.ssh_multiplex);
        assert!(!policy.availability.multiplex);
        assert!(policy.support.reconnect);
        assert!(policy.availability.reconnect);
        assert!(policy.support.disconnect);
        assert!(!policy.availability.disconnect);
        assert!(policy.support.ai);
        assert!(!policy.availability.use_ai);
    }

    #[test]
    fn busy_and_terminal_unavailable_disable_runtime_actions_without_hiding_them() {
        let mut context = input(TabSessionCapability::Ssh);
        context.is_busy = true;
        context.terminal_available = false;
        let policy = TabActionPolicy::from_input(context);

        assert!(policy.support.ssh_multiplex);
        assert!(!policy.availability.multiplex);
        assert!(!policy.availability.reconnect);
        assert!(policy.support.disconnect);
        assert!(!policy.availability.disconnect);
        assert!(policy.support.ai);
        assert!(!policy.availability.use_ai);
    }

    #[test]
    fn reconnect_pending_disables_reconnect_but_preserves_visibility() {
        let mut context = input(TabSessionCapability::Telnet);
        context.is_disconnected = true;
        context.reconnect_pending = true;
        let policy = TabActionPolicy::from_input(context);

        assert!(policy.support.reconnect);
        assert!(!policy.availability.reconnect);
    }

    #[test]
    fn split_group_is_available_for_terminal_sessions() {
        let plain = TabActionPolicy::from_input(input(TabSessionCapability::Local));
        assert!(plain.support.split);
        assert!(plain.availability.split);
    }

    #[test]
    fn locked_and_tab_position_only_change_close_availability() {
        let mut first = input(TabSessionCapability::Local);
        first.locked = true;
        first.tab_count = 3;
        first.tab_index = Some(0);
        let first = TabActionPolicy::from_input(first);
        assert!(!first.availability.close_tab);
        assert!(first.availability.close_inactive);
        assert!(first.availability.close_right);

        let mut middle = input(TabSessionCapability::Local);
        middle.tab_count = 3;
        middle.tab_index = Some(1);
        assert!(TabActionPolicy::from_input(middle).availability.close_right);

        let mut last = input(TabSessionCapability::Local);
        last.tab_count = 3;
        last.tab_index = Some(2);
        assert!(!TabActionPolicy::from_input(last).availability.close_right);
    }

    #[test]
    fn menu_groups_never_create_empty_or_adjacent_separator_slots() {
        let remote = TabActionPolicy::from_input(input(TabSessionCapability::Vnc));
        assert_eq!(
            remote.menu_groups(),
            vec![TabActionMenuGroup::General, TabActionMenuGroup::Close]
        );

        let terminal = TabActionPolicy::from_input(input(TabSessionCapability::Ssh));
        let groups = terminal.menu_groups();
        assert_eq!(groups.first(), Some(&TabActionMenuGroup::General));
        assert_eq!(groups.last(), Some(&TabActionMenuGroup::Close));
        assert!(groups.windows(2).all(|pair| pair[0] != pair[1]));
        assert_eq!(groups.len() - 1, 3);
    }
    #[test]
    fn android_screen_is_protocol_specific_and_disabled_during_disconnect_or_connection_work() {
        for session in [
            TabSessionCapability::Local,
            TabSessionCapability::Ssh,
            TabSessionCapability::Telnet,
            TabSessionCapability::Serial,
            TabSessionCapability::Rdp,
            TabSessionCapability::Vnc,
        ] {
            assert!(
                !TabActionPolicy::from_input(input(session))
                    .support
                    .android_screen
            );
        }
        for session in [
            TabSessionCapability::AdbShell,
            TabSessionCapability::AdbView,
        ] {
            let context = input(session);
            assert!(
                TabActionPolicy::from_input(context)
                    .availability
                    .android_screen
            );
            for condition in 0..3 {
                let mut unavailable = context;
                unavailable.is_busy = condition == 0;
                unavailable.is_disconnected = condition == 1;
                unavailable.reconnect_pending = condition == 2;
                let policy = TabActionPolicy::from_input(unavailable);
                assert!(policy.support.android_screen);
                assert!(!policy.availability.android_screen);
            }
        }
    }
}
