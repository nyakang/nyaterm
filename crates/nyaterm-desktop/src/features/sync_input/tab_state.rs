//! Read-only tab projection. Eligibility is deliberately independent of group pauses.

use super::SyncInputFeatureState;

#[derive(Clone, Debug)]
pub(in crate::features) struct TabSyncGroup {
    pub id: String,
    pub name: String,
    pub color: u32,
    pub enabled: bool,
    pub paused: bool,
}

#[derive(Clone, Debug)]
pub(in crate::features) struct TabSyncPane {
    pub id: String,
    pub receives: bool,
    pub sends: bool,
    pub groups: Vec<TabSyncGroup>,
}

#[derive(Clone, Debug)]
pub(in crate::features) struct TabSyncState {
    pub broadcast: bool,
    pub panes: Vec<TabSyncPane>,
}

impl SyncInputFeatureState {
    pub(in crate::features) fn tab_sync_state(&self, ids: &[String]) -> Option<TabSyncState> {
        if !self.input_selection_active() || ids.is_empty() {
            return None;
        }
        Some(TabSyncState {
            broadcast: self.broadcast_to_all(),
            panes: ids
                .iter()
                .map(|id| TabSyncPane {
                    id: id.clone(),
                    receives: self.session_receives_input(id),
                    sends: self.may_fan_out_from(id),
                    groups: self
                        .groups()
                        .iter()
                        .filter(|group| group.session_ids.contains(id))
                        .map(|group| TabSyncGroup {
                            id: group.id.clone(),
                            name: group.name.clone(),
                            color: group.color,
                            enabled: group.enabled,
                            paused: group.paused_session_ids.contains(id),
                        })
                        .collect(),
                })
                .collect(),
        })
    }
}

impl TabSyncState {
    pub fn receive_count(&self) -> usize {
        self.panes.iter().filter(|pane| pane.receives).count()
    }

    pub fn all_receive(&self) -> bool {
        !self.panes.is_empty() && self.receive_count() == self.panes.len()
    }

    pub fn mixed_receive(&self) -> bool {
        self.receive_count() > 0 && !self.all_receive()
    }

    pub fn ungrouped(&self) -> bool {
        !self.broadcast
            && self
                .panes
                .iter()
                .all(|pane| pane.groups.iter().all(|group| !group.enabled))
    }

    /// A group color describes the whole tab only if every pane has exactly this group.
    pub fn common_group(&self) -> Option<&TabSyncGroup> {
        let first = self
            .panes
            .first()?
            .groups
            .iter()
            .find(|group| group.enabled)?;
        self.panes
            .iter()
            .all(|pane| {
                let mut groups = pane.groups.iter().filter(|group| group.enabled);
                groups.next().is_some_and(|group| group.id == first.id) && groups.next().is_none()
            })
            .then_some(first)
    }

    pub fn mixed_membership(&self) -> bool {
        !self.broadcast && !self.ungrouped() && self.common_group().is_none()
    }

    pub fn has_group_pause(&self) -> bool {
        self.panes.iter().any(|pane| {
            pane.groups
                .iter()
                .any(|group| group.enabled && group.paused)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::SyncInputFeatureState;
    use crate::models::SyncInputGroup;
    use gpui::TestAppContext;

    fn fixture() -> SyncInputFeatureState {
        let cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let focus = cx.update(|cx| cx.focus_handle());
        let mut state = SyncInputFeatureState::new(focus);
        state.groups.push(SyncInputGroup {
            id: "group-a".into(),
            name: "A".into(),
            color: 0x123456,
            enabled: true,
            session_ids: vec!["a".into(), "b".into()],
            paused_session_ids: vec!["b".into()],
        });
        state
    }

    #[test]
    fn eligibility_pause_and_sending_remain_independent() {
        let mut state = fixture();
        let ids = vec!["a".into(), "b".into()];
        let tab = state.tab_sync_state(&ids).unwrap();
        assert!(tab.all_receive());
        assert!(tab.has_group_pause());
        assert!(tab.panes[0].sends);
        assert!(!tab.panes[1].sends);
        state.set_session_receives_input("a", false);
        let tab = state.tab_sync_state(&ids).unwrap();
        assert!(tab.mixed_receive());
        assert!(tab.panes[0].sends);
        assert!(tab.common_group().is_some());
        state.set_session_receives_input("b", false);
        let tab = state.tab_sync_state(&ids).unwrap();
        assert_eq!(tab.receive_count(), 0);
        assert!(!tab.ungrouped());
        assert!(tab.has_group_pause());
    }

    #[test]
    fn mixed_membership_never_borrows_the_first_panes_color() {
        let mut state = fixture();
        let ids = vec!["a".into(), "outside".into()];
        let tab = state.tab_sync_state(&ids).unwrap();
        assert!(tab.mixed_membership());
        assert!(tab.common_group().is_none());
        let mut second = state.groups[0].clone();
        second.id = "group-b".into();
        second.session_ids = vec!["outside".into()];
        state.groups.push(second);
        let tab = state.tab_sync_state(&ids).unwrap();
        assert!(tab.all_receive());
        assert!(tab.mixed_membership());
        state.groups[1].session_ids.push("a".into());
        let tab = state.tab_sync_state(&["a".into()]).unwrap();
        assert!(tab.mixed_membership());
        assert_eq!(tab.panes[0].groups.len(), 2);
    }

    #[test]
    fn broadcast_overrides_routes_but_retains_pause_and_exclusion_information() {
        let mut state = fixture();
        state.set_session_receives_input("b", false);
        state.toggle_broadcast_to_all();
        let tab = state
            .tab_sync_state(&["b".into(), "outside".into()])
            .unwrap();
        assert!(tab.broadcast);
        assert!(tab.mixed_receive());
        assert!(tab.has_group_pause());
        assert!(tab.panes.iter().all(|pane| pane.sends));
        assert!(!tab.ungrouped());
        assert!(!tab.mixed_membership());
    }

    #[test]
    fn disabled_groups_and_empty_tabs_do_not_offer_controls() {
        let mut state = fixture();
        assert!(state.tab_sync_state(&[]).is_none());
        assert!(
            state
                .tab_sync_state(&["outside".into()])
                .unwrap()
                .ungrouped()
        );
        state.groups[0].enabled = false;
        assert!(state.tab_sync_state(&["a".into()]).is_none());
        state.toggle_broadcast_to_all();
        let tab = state.tab_sync_state(&["a".into()]).unwrap();
        assert!(tab.all_receive());
        assert!(!tab.panes[0].groups[0].enabled);
    }
}
