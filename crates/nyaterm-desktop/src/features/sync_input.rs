use std::collections::HashSet;

use gpui::{Context, FocusHandle, Window};
use nyaterm_core::uuid;
use nyaterm_transport::SessionInfo;
use nyaterm_ui::ChildWindowSlot;

mod window;

use super::NyaTermApp;
use crate::models::SyncInputGroup;

const GROUP_COLORS: [u32; 8] = [
    0x3b82f6, 0xef4444, 0x22c55e, 0xf59e0b, 0x8b5cf6, 0xec4899, 0x06b6d4, 0xf97316,
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::features) enum SyncGroupSessionFilter {
    #[default]
    All,
    Selected,
    Available,
}

pub(in crate::features) struct SyncInputFeatureState {
    groups: Vec<SyncInputGroup>,
    window: ChildWindowSlot,
    restore_focus: Option<FocusHandle>,
    focus: FocusHandle,
    search_draft: String,
    session_filter: SyncGroupSessionFilter,
    selected_id: Option<String>,
    delete_pending: Option<String>,
    broadcast_to_all: bool,
    /// Runtime target veto across every route; never restricts a source.
    receive_excluded: HashSet<String>,
    // Runtime-only: even toggling routing on and back off invalidates old fills.
    revision: u64,
}

pub(in crate::features) enum SyncSessionPauseResult {
    Paused,
    Resumed,
    NotMember,
    NoGroup,
}

pub(in crate::features) struct SyncInputTransferBundle {
    original_groups: Vec<SyncInputGroup>,
    moved_groups: Vec<SyncInputGroup>,
    moved_excluded: HashSet<String>,
}

impl SyncInputFeatureState {
    pub(in crate::features) fn revision(&self) -> u64 {
        self.revision
    }

    fn note_routing_change(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    pub(in crate::features) fn detach_sessions_for_transfer(
        &mut self,
        session_ids: &[String],
    ) -> SyncInputTransferBundle {
        self.note_routing_change();
        let original_groups = self.groups.clone();
        let moved_excluded = self
            .receive_excluded
            .extract_if(|id| session_ids.contains(id))
            .collect();
        let mut moved_groups = Vec::new();
        for group in &mut self.groups {
            let mut moved = group.clone();
            moved.session_ids.retain(|id| session_ids.contains(id));
            moved
                .paused_session_ids
                .retain(|id| session_ids.contains(id));
            if !moved.session_ids.is_empty() {
                moved_groups.push(moved);
            }
            group.session_ids.retain(|id| !session_ids.contains(id));
            group
                .paused_session_ids
                .retain(|id| !session_ids.contains(id));
        }
        self.groups.retain(|group| !group.session_ids.is_empty());
        self.repair_selection();
        SyncInputTransferBundle {
            original_groups,
            moved_groups,
            moved_excluded,
        }
    }

    pub(in crate::features) fn attach_sessions_from_transfer(
        &mut self,
        bundle: SyncInputTransferBundle,
    ) {
        self.note_routing_change();
        self.receive_excluded.extend(bundle.moved_excluded);
        for mut group in bundle.moved_groups {
            if self.groups.iter().any(|existing| existing.id == group.id) {
                group.id = format!("sync-group-{}", uuid());
            }
            self.groups.push(group);
        }
        self.repair_selection();
    }

    pub(in crate::features) fn restore_sessions_after_failed_transfer(
        &mut self,
        bundle: SyncInputTransferBundle,
    ) {
        self.note_routing_change();
        self.groups = bundle.original_groups;
        self.receive_excluded.extend(bundle.moved_excluded);
        self.repair_selection();
    }

    pub(in crate::features) fn retains_transfer_session(&self, id: &str) -> bool {
        self.receive_excluded.contains(id)
            || self
                .groups
                .iter()
                .any(|group| group.session_ids.iter().any(|member| member == id))
    }

    pub(in crate::features) fn new(focus: FocusHandle) -> Self {
        Self {
            groups: Vec::new(),
            window: ChildWindowSlot::default(),
            restore_focus: None,
            focus,
            search_draft: String::new(),
            session_filter: SyncGroupSessionFilter::All,
            selected_id: None,
            delete_pending: None,
            broadcast_to_all: false,
            receive_excluded: HashSet::new(),
            revision: 0,
        }
    }

    fn request_open(&mut self) -> bool {
        if !self.window.begin_open() {
            return false;
        }
        self.search_draft.clear();
        self.session_filter = SyncGroupSessionFilter::All;
        self.delete_pending = None;
        if self.selected_id.is_none() {
            self.selected_id = self.groups.first().map(|group| group.id.clone());
        }
        true
    }

    pub(in crate::features) fn groups(&self) -> &[SyncInputGroup] {
        &self.groups
    }

    pub(in crate::features) fn window_is_open_or_pending(&self) -> bool {
        self.window.is_open_or_pending()
    }

    pub(in crate::features) fn focus(&self) -> &FocusHandle {
        &self.focus
    }

    pub(in crate::features) fn search_draft(&self) -> &str {
        &self.search_draft
    }

    pub(in crate::features) fn session_filter(&self) -> SyncGroupSessionFilter {
        self.session_filter
    }

    pub(in crate::features) fn set_session_filter(&mut self, filter: SyncGroupSessionFilter) {
        self.session_filter = filter;
    }

    pub(in crate::features) fn pending_delete_group(&self) -> Option<&SyncInputGroup> {
        self.delete_pending
            .as_deref()
            .and_then(|id| self.groups.iter().find(|group| group.id == id))
    }

    pub(in crate::features) fn broadcast_to_all(&self) -> bool {
        self.broadcast_to_all
    }

    pub(in crate::features) fn close(&mut self) {
        self.window.clear();
        self.search_draft.clear();
        self.session_filter = SyncGroupSessionFilter::All;
        self.delete_pending = None;
    }

    pub(in crate::features) fn create_group(&mut self) {
        self.note_routing_change();
        let index = self.groups.len();
        let group = SyncInputGroup {
            id: format!("sync-group-{}", uuid()),
            name: format!("Sync Group {}", index + 1),
            color: self.next_group_color(),
            session_ids: Vec::new(),
            paused_session_ids: Vec::new(),
            enabled: true,
        };
        self.selected_id = Some(group.id.clone());
        self.groups.push(group);
    }

    pub(in crate::features) fn ensure_selected_group(&mut self) {
        if self.selected_group().is_some() {
            return;
        }
        if let Some(first_id) = self.groups.first().map(|group| group.id.clone()) {
            self.selected_id = Some(first_id);
        } else {
            self.create_group();
        }
    }

    pub(in crate::features) fn cycle_selected_color(&mut self) -> Option<u32> {
        let group = self.selected_group_mut()?;
        let current_idx = GROUP_COLORS
            .iter()
            .position(|color| *color == group.color)
            .unwrap_or(0);
        let next_color = GROUP_COLORS[(current_idx + 1) % GROUP_COLORS.len()];
        group.color = next_color;
        Some(next_color)
    }

    pub(in crate::features) fn other_group_names_for_session(
        &self,
        session_id: &str,
    ) -> Vec<String> {
        let selected_id = self.selected_id.as_deref();
        self.groups
            .iter()
            .filter(|group| {
                Some(group.id.as_str()) != selected_id
                    && group.session_ids.iter().any(|id| id == session_id)
            })
            .map(|group| group.name.clone())
            .collect()
    }

    pub(in crate::features) fn delete_selected(&mut self) -> bool {
        self.note_routing_change();
        let Some(group_id) = self.selected_id.clone() else {
            return false;
        };
        self.groups.retain(|group| group.id != group_id);
        self.selected_id = self.groups.first().map(|group| group.id.clone());
        true
    }

    pub(in crate::features) fn request_delete_selected(&mut self) -> bool {
        let Some(group_id) = self.selected_id.clone() else {
            return false;
        };
        self.delete_pending = Some(group_id);
        true
    }

    pub(in crate::features) fn confirm_delete(&mut self) -> bool {
        let Some(group_id) = self.delete_pending.take() else {
            return false;
        };
        self.selected_id = Some(group_id);
        self.delete_selected()
    }

    pub(in crate::features) fn cancel_delete(&mut self) {
        self.delete_pending = None;
    }

    pub(in crate::features) fn set_search(&mut self, text: String) {
        self.search_draft = text;
    }

    pub(in crate::features) fn rename_group(&mut self, group_id: &str, text: String) -> bool {
        let Some(group) = self.groups.iter_mut().find(|group| group.id == group_id) else {
            return false;
        };
        group.name = text;
        true
    }

    pub(in crate::features) fn select(&mut self, group_id: String) -> bool {
        if !self.groups.iter().any(|group| group.id == group_id) {
            return false;
        }
        self.selected_id = Some(group_id);
        true
    }

    pub(in crate::features) fn selected_group(&self) -> Option<&SyncInputGroup> {
        self.selected_id
            .as_deref()
            .and_then(|id| self.groups.iter().find(|group| group.id == id))
    }

    fn selected_group_mut(&mut self) -> Option<&mut SyncInputGroup> {
        let selected_id = self.selected_id.clone()?;
        self.groups.iter_mut().find(|group| group.id == selected_id)
    }

    pub(in crate::features) fn replace_selected_sessions(
        &mut self,
        session_ids: Vec<String>,
    ) -> bool {
        self.note_routing_change();
        let Some(group) = self.selected_group_mut() else {
            return false;
        };
        group.session_ids = session_ids;
        group
            .paused_session_ids
            .retain(|id| group.session_ids.iter().any(|member| member == id));
        true
    }

    pub(in crate::features) fn clear_selected_sessions(&mut self) -> bool {
        self.replace_selected_sessions(Vec::new())
    }

    pub(in crate::features) fn add_selected_sessions(&mut self, session_ids: Vec<String>) -> bool {
        self.note_routing_change();
        let Some(group) = self.selected_group_mut() else {
            return false;
        };
        for session_id in session_ids {
            if !group.session_ids.iter().any(|id| id == &session_id) {
                group.session_ids.push(session_id);
            }
        }
        true
    }

    pub(in crate::features) fn remove_selected_sessions(
        &mut self,
        remove_ids: &HashSet<String>,
    ) -> bool {
        self.note_routing_change();
        let Some(group) = self.selected_group_mut() else {
            return false;
        };
        group.session_ids.retain(|id| !remove_ids.contains(id));
        group
            .paused_session_ids
            .retain(|id| !remove_ids.contains(id));
        true
    }

    pub(in crate::features) fn toggle_selected_enabled(&mut self) -> Option<bool> {
        self.note_routing_change();
        let group = self.selected_group_mut()?;
        group.enabled = !group.enabled;
        Some(group.enabled)
    }

    /// Returns `Some(true)` when added and `Some(false)` when removed.
    pub(in crate::features) fn toggle_selected_session(
        &mut self,
        session_id: String,
    ) -> Option<bool> {
        self.note_routing_change();
        let group = self.selected_group_mut()?;
        if group.session_ids.iter().any(|id| id == &session_id) {
            group.session_ids.retain(|id| id != &session_id);
            group.paused_session_ids.retain(|id| id != &session_id);
            Some(false)
        } else {
            group.session_ids.push(session_id);
            Some(true)
        }
    }

    pub(in crate::features) fn toggle_selected_session_paused(
        &mut self,
        session_id: String,
    ) -> SyncSessionPauseResult {
        self.note_routing_change();
        let Some(group) = self.selected_group_mut() else {
            return SyncSessionPauseResult::NoGroup;
        };
        if !group.session_ids.iter().any(|id| id == &session_id) {
            return SyncSessionPauseResult::NotMember;
        }
        if group.paused_session_ids.iter().any(|id| id == &session_id) {
            group.paused_session_ids.retain(|id| id != &session_id);
            SyncSessionPauseResult::Resumed
        } else {
            group.paused_session_ids.push(session_id);
            SyncSessionPauseResult::Paused
        }
    }

    pub(in crate::features) fn active_group_for_session(
        &self,
        session_id: &str,
    ) -> Option<&SyncInputGroup> {
        self.groups
            .iter()
            .find(|group| group.enabled && group.session_ids.iter().any(|id| id == session_id))
    }

    pub(in crate::features) fn session_is_paused_in_active_group(&self, session_id: &str) -> bool {
        self.active_group_for_session(session_id)
            .is_some_and(|group| group.paused_session_ids.iter().any(|id| id == session_id))
    }

    pub(in crate::features) fn peer_session_ids(
        &self,
        session_id: &str,
        live_ids: &HashSet<String>,
    ) -> Vec<String> {
        let mut peers = HashSet::new();
        for group in &self.groups {
            if !group.enabled
                || !group.session_ids.iter().any(|id| id == session_id)
                || group.paused_session_ids.iter().any(|id| id == session_id)
            {
                continue;
            }
            for peer_id in &group.session_ids {
                if peer_id != session_id
                    && live_ids.contains(peer_id)
                    && !group.paused_session_ids.iter().any(|id| id == peer_id)
                {
                    peers.insert(peer_id.clone());
                }
            }
        }
        if self.broadcast_to_all {
            for peer_id in live_ids {
                if peer_id != session_id {
                    peers.insert(peer_id.clone());
                }
            }
        }
        peers.retain(|peer_id| !self.receive_excluded.contains(peer_id));
        let mut peers = peers.into_iter().collect::<Vec<_>>();
        peers.sort();
        peers
    }

    pub(in crate::features) fn may_fan_out_from(&self, session_id: &str) -> bool {
        if self.broadcast_to_all {
            return true;
        }
        self.groups.iter().any(|group| {
            group.enabled
                && group.session_ids.iter().any(|id| id == session_id)
                && !group.paused_session_ids.iter().any(|id| id == session_id)
        })
    }

    pub(in crate::features) fn toggle_broadcast_to_all(&mut self) -> bool {
        self.note_routing_change();
        self.broadcast_to_all = !self.broadcast_to_all;
        self.broadcast_to_all
    }

    /// True while any session can mirror input, so tabs offer the per-session checkbox.
    pub(in crate::features) fn input_selection_active(&self) -> bool {
        self.broadcast_to_all || self.groups.iter().any(|group| group.enabled)
    }

    /// Tab target eligibility. Group pause is an independent control, shown separately.
    pub(in crate::features) fn session_receives_input(&self, session_id: &str) -> bool {
        self.target_allowed(session_id)
            && (self.broadcast_to_all || self.active_group_for_session(session_id).is_some())
    }

    pub(in crate::features) fn target_allowed(&self, session_id: &str) -> bool {
        !self.receive_excluded.contains(session_id)
    }

    /// Returns (all, mixed) for the terminal panes of a tab.
    pub(in crate::features) fn tab_receive_state(&self, session_ids: &[String]) -> (bool, bool) {
        let selected = session_ids
            .iter()
            .filter(|id| self.session_receives_input(id))
            .count();
        (
            !session_ids.is_empty() && selected == session_ids.len(),
            selected > 0 && selected < session_ids.len(),
        )
    }

    /// Ticking a nonmember joins the selected enabled group, or the first enabled
    /// group. Existing membership and group-specific pause state are preserved.
    pub(in crate::features) fn set_session_receives_input(
        &mut self,
        session_id: &str,
        receive: bool,
    ) -> bool {
        let mut joined = false;
        if receive && !self.broadcast_to_all && self.active_group_for_session(session_id).is_none()
        {
            let target_id = self
                .selected_group()
                .filter(|group| group.enabled)
                .or_else(|| self.groups.iter().find(|group| group.enabled))
                .map(|group| group.id.clone());
            let Some(target_id) = target_id else {
                return false;
            };
            let group = self
                .groups
                .iter_mut()
                .find(|group| group.id == target_id)
                .unwrap();
            group.session_ids.push(session_id.to_string());
            joined = true;
        }
        let changed = if receive {
            self.receive_excluded.remove(session_id)
        } else {
            self.receive_excluded.insert(session_id.to_string())
        } || joined;
        if changed {
            self.note_routing_change();
        }
        changed
    }

    pub(in crate::features) fn toggle_active_session_paused(
        &mut self,
        session_id: String,
    ) -> SyncSessionPauseResult {
        self.note_routing_change();
        let Some(group_id) = self
            .active_group_for_session(&session_id)
            .map(|group| group.id.clone())
        else {
            return SyncSessionPauseResult::NoGroup;
        };
        let Some(group) = self.groups.iter_mut().find(|group| group.id == group_id) else {
            return SyncSessionPauseResult::NoGroup;
        };
        if group.paused_session_ids.iter().any(|id| id == &session_id) {
            group.paused_session_ids.retain(|id| id != &session_id);
            SyncSessionPauseResult::Resumed
        } else {
            group.paused_session_ids.push(session_id);
            SyncSessionPauseResult::Paused
        }
    }

    pub(in crate::features) fn leave_active_group(&mut self, session_id: &str) -> bool {
        self.note_routing_change();
        let Some(group_id) = self
            .active_group_for_session(session_id)
            .map(|group| group.id.clone())
        else {
            return false;
        };
        if let Some(group) = self.groups.iter_mut().find(|group| group.id == group_id) {
            group.session_ids.retain(|id| id != session_id);
            group.paused_session_ids.retain(|id| id != session_id);
        }
        self.groups.retain(|group| !group.session_ids.is_empty());
        self.repair_selection();
        true
    }

    pub(in crate::features) fn close_active_group(&mut self, session_id: &str) -> bool {
        self.note_routing_change();
        let Some(group_id) = self
            .active_group_for_session(session_id)
            .map(|group| group.id.clone())
        else {
            return false;
        };
        let Some(group) = self.groups.iter_mut().find(|group| group.id == group_id) else {
            return false;
        };
        group.enabled = false;
        true
    }

    pub(in crate::features) fn purge_session(&mut self, session_id: &str) {
        self.note_routing_change();
        self.receive_excluded.remove(session_id);
        for group in &mut self.groups {
            group.session_ids.retain(|id| id != session_id);
            group.paused_session_ids.retain(|id| id != session_id);
        }
        self.groups.retain(|group| !group.session_ids.is_empty());
        self.repair_selection();
    }

    pub(in crate::features) fn replace_session_id(&mut self, old_id: &str, new_id: &str) {
        self.note_routing_change();
        if self.receive_excluded.remove(old_id) {
            self.receive_excluded.insert(new_id.to_string());
        }
        for group in &mut self.groups {
            for session_id in &mut group.session_ids {
                if session_id == old_id {
                    *session_id = new_id.to_string();
                }
            }
            if group.paused_session_ids.iter().any(|id| id == old_id) {
                group.paused_session_ids.retain(|id| id != old_id);
                if !group.paused_session_ids.iter().any(|id| id == new_id) {
                    group.paused_session_ids.push(new_id.to_string());
                }
            }
        }
    }

    fn repair_selection(&mut self) {
        if self
            .selected_id
            .as_deref()
            .is_some_and(|id| !self.groups.iter().any(|group| group.id == id))
        {
            self.selected_id = self.groups.first().map(|group| group.id.clone());
        }
    }

    fn next_group_color(&self) -> u32 {
        GROUP_COLORS
            .iter()
            .copied()
            .find(|color| self.groups.iter().all(|group| group.color != *color))
            .unwrap_or(GROUP_COLORS[self.groups.len() % GROUP_COLORS.len()])
    }
}

impl NyaTermApp {
    pub(in crate::features) fn close_sync_groups(&mut self, cx: &mut Context<Self>) {
        self.sync_input.close();
        self.forget_text_inputs("sync.groups.search");
        self.forget_text_inputs("sync.group-name.");
        self.shell.set_status("sync groups closed".to_string());
        let restore_focus = self.sync_input.restore_focus.take();
        if let Some(parent) = self.shell.main_window() {
            let app = cx.weak_entity();
            cx.defer(move |cx| {
                let _ = parent.update(cx, |_, window, cx| {
                    window.activate_window();
                    if let Some(focus) = restore_focus {
                        window.focus(&focus, cx);
                    } else {
                        let _ = app.update(cx, |app, cx| {
                            app.focus_active_workspace_surface(window, cx);
                        });
                    }
                });
            });
        }
        cx.notify();
    }

    pub(in crate::features) fn create_sync_group(&mut self, cx: &mut Context<Self>) {
        self.sync_input.create_group();
        self.shell.set_status("sync group created".to_string());
        cx.notify();
    }

    pub(in crate::features) fn cycle_selected_sync_group_color(&mut self, cx: &mut Context<Self>) {
        if self.sync_input.cycle_selected_color().is_some() {
            cx.notify();
        }
    }

    pub(in crate::features) fn set_sync_groups_session_filter(
        &mut self,
        filter: SyncGroupSessionFilter,
        cx: &mut Context<Self>,
    ) {
        self.sync_input.set_session_filter(filter);
        cx.notify();
    }

    pub(in crate::features) fn request_delete_selected_sync_group(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.sync_input.request_delete_selected() {
            self.shell.set_status("no sync group selected".to_string());
            cx.notify();
            return;
        }
        let Some(group) = self.sync_input.pending_delete_group() else {
            return;
        };
        let message = rust_i18n::t!("syncGroup.deleteGroupConfirm", name = &group.name).to_string();
        self.open_confirm_dialog_with_cancel(
            (
                rust_i18n::t!("syncGroup.deleteGroup").to_string(),
                message,
                rust_i18n::t!("syncGroup.deleteGroup").to_string(),
                true,
                |app, _, cx| {
                    app.confirm_delete_sync_group(cx);
                    true
                },
                |app, cx| app.cancel_delete_sync_group(cx),
            ),
            window,
            cx,
        );
        cx.notify();
    }

    pub(in crate::features) fn cancel_delete_sync_group(&mut self, cx: &mut Context<Self>) {
        self.sync_input.cancel_delete();
        cx.notify();
    }

    pub(in crate::features) fn confirm_delete_sync_group(&mut self, cx: &mut Context<Self>) {
        if !self.sync_input.confirm_delete() {
            return;
        }
        self.shell.set_status("sync group deleted".to_string());
        cx.notify();
    }

    pub(in crate::features) fn apply_sync_groups_search(
        &mut self,
        text: String,
        cx: &mut Context<Self>,
    ) {
        self.sync_input.set_search(text);
        cx.notify();
    }

    pub(in crate::features) fn apply_sync_group_name(
        &mut self,
        group_id: &str,
        text: String,
        cx: &mut Context<Self>,
    ) {
        if self.sync_input.rename_group(group_id, text) {
            cx.notify();
        }
    }

    pub(in crate::features) fn select_all_sync_group_sessions(&mut self, cx: &mut Context<Self>) {
        let session_ids = self
            .session
            .ordered_sessions()
            .into_iter()
            .map(|session| session.id)
            .collect::<Vec<_>>();
        if session_ids.is_empty() {
            return;
        }
        self.sync_input.ensure_selected_group();
        if self.sync_input.replace_selected_sessions(session_ids) {
            cx.notify();
        }
    }

    pub(in crate::features) fn clear_sync_group_sessions(&mut self, cx: &mut Context<Self>) {
        if self.sync_input.clear_selected_sessions() {
            cx.notify();
        }
    }

    pub(in crate::features) fn add_filtered_sync_group_sessions(&mut self, cx: &mut Context<Self>) {
        let query = self.sync_input.search_draft().trim().to_ascii_lowercase();
        let session_ids = self
            .session
            .ordered_sessions()
            .into_iter()
            .filter(|session| self.sync_group_session_matches_search(session, &query))
            .map(|session| session.id)
            .collect::<Vec<_>>();
        if session_ids.is_empty() {
            return;
        }
        self.sync_input.ensure_selected_group();
        if self.sync_input.add_selected_sessions(session_ids) {
            cx.notify();
        }
    }

    pub(in crate::features) fn remove_filtered_sync_group_sessions(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let query = self.sync_input.search_draft().trim().to_ascii_lowercase();
        let remove_ids = self
            .session
            .ordered_sessions()
            .into_iter()
            .filter(|session| self.sync_group_session_matches_search(session, &query))
            .map(|session| session.id)
            .collect::<HashSet<_>>();
        if self.sync_input.remove_selected_sessions(&remove_ids) {
            cx.notify();
        }
    }

    pub(in crate::features) fn select_same_host_sync_group_sessions(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let selected_ids = self
            .sync_input
            .selected_group()
            .map(|group| group.session_ids.clone())
            .unwrap_or_default();
        let selected_hosts = selected_ids
            .iter()
            .filter_map(|id| self.session.ssh_host(id))
            .collect::<HashSet<_>>();
        if selected_hosts.is_empty() {
            return;
        }
        let matching_ids = self
            .session
            .ordered_sessions()
            .into_iter()
            .filter(|session| {
                self.session
                    .ssh_host(&session.id)
                    .is_some_and(|host| selected_hosts.contains(&host))
            })
            .map(|session| session.id)
            .collect::<Vec<_>>();
        if self.sync_input.replace_selected_sessions(matching_ids) {
            cx.notify();
        }
    }

    pub(in crate::features) fn sync_group_session_matches_search(
        &self,
        session: &SessionInfo,
        query: &str,
    ) -> bool {
        if query.is_empty() {
            return true;
        }
        let endpoint = self.session.endpoint(&session.id).unwrap_or_default();
        let host = self.session.ssh_host(&session.id).unwrap_or_default();
        format!(
            "{} {:?} {} {} {}",
            self.session.display_name_by_info(session),
            session.kind,
            endpoint,
            host,
            session.id
        )
        .to_ascii_lowercase()
        .contains(query)
    }

    pub(in crate::features) fn select_sync_group(
        &mut self,
        group_id: String,
        cx: &mut Context<Self>,
    ) {
        if self.sync_input.select(group_id) {
            self.shell.set_status("sync group selected".to_string());
            cx.notify();
        }
    }

    pub(in crate::features) fn toggle_selected_sync_group_enabled(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let Some(enabled) = self.sync_input.toggle_selected_enabled() else {
            self.shell.set_status("no sync group selected".to_string());
            cx.notify();
            return;
        };
        self.shell.set_status(if enabled {
            "sync group enabled".to_string()
        } else {
            "sync group disabled".to_string()
        });
        cx.notify();
    }

    pub(in crate::features) fn toggle_session_in_selected_sync_group(
        &mut self,
        session_id: String,
        cx: &mut Context<Self>,
    ) {
        self.sync_input.ensure_selected_group();
        let Some(added) = self.sync_input.toggle_selected_session(session_id) else {
            self.shell
                .set_status("create or select a sync group first".to_string());
            cx.notify();
            return;
        };
        if added {
            self.shell
                .set_status("session added to sync group".to_string());
        } else {
            self.shell
                .set_status("session removed from sync group".to_string());
        }
        cx.notify();
    }

    pub(in crate::features) fn toggle_session_paused_in_selected_sync_group(
        &mut self,
        session_id: String,
        cx: &mut Context<Self>,
    ) {
        self.shell.set_status(
            match self.sync_input.toggle_selected_session_paused(session_id) {
                SyncSessionPauseResult::Paused => "session sync paused",
                SyncSessionPauseResult::Resumed => "session sync resumed",
                SyncSessionPauseResult::NotMember => "session is not in the selected sync group",
                SyncSessionPauseResult::NoGroup => "create or select a sync group first",
            }
            .to_string(),
        );
        cx.notify();
    }

    /// Metadata is authoritative for both terminal and helper-backed sessions.
    pub(in crate::features) fn terminal_sync_session(&self, session_id: &str) -> bool {
        self.session.metadata(session_id).is_some_and(|metadata| {
            !matches!(
                metadata.launch_config,
                crate::models::SessionLaunchConfig::Rdp(_)
                    | crate::models::SessionLaunchConfig::Vnc(_)
            )
        })
    }

    pub(in crate::features) fn sync_target_allowed(&self, session_id: &str) -> bool {
        self.terminal_sync_session(session_id)
            && !self.session.is_disconnected(session_id)
            && self.sync_input.target_allowed(session_id)
    }

    pub(in crate::features) fn sync_peer_session_ids(&self, session_id: &str) -> Vec<String> {
        if !self.terminal_sync_session(session_id)
            || self.session.is_disconnected(session_id)
            || !self.sync_input.may_fan_out_from(session_id)
        {
            return Vec::new();
        }
        let live_ids = self
            .session
            .metadata_entries()
            .filter(|(id, metadata)| !metadata.disconnected && self.terminal_sync_session(id))
            .map(|(session_id, _)| session_id.to_string())
            .collect::<HashSet<_>>();
        self.sync_input.peer_session_ids(session_id, &live_ids)
    }

    pub(in crate::features) fn toggle_broadcast_to_all(&mut self, cx: &mut Context<Self>) {
        self.shell
            .set_status(if self.sync_input.toggle_broadcast_to_all() {
                "broadcast to all sessions enabled".to_string()
            } else {
                "broadcast to all sessions disabled".to_string()
            });
        cx.notify();
    }

    /// Tab checkbox: tick or untick every pane under one tab in a single step.
    pub(in crate::features) fn set_sessions_receive_sync_input(
        &mut self,
        session_ids: &[String],
        receive: bool,
        cx: &mut Context<Self>,
    ) {
        let mut changed = false;
        for session_id in session_ids {
            if !self.terminal_sync_session(session_id) {
                continue;
            }
            if !receive {
                self.commands.cancel_staged_sync_receiver(session_id);
            }
            changed |= self
                .sync_input
                .set_session_receives_input(session_id, receive);
        }
        if !changed {
            return;
        }
        self.shell.set_status(if receive {
            "session sync resumed".to_string()
        } else {
            "session sync paused".to_string()
        });
        cx.notify();
    }

    /// Pause/resume the current session inside its active enabled sync group.
    pub(in crate::features) fn toggle_session_paused_in_active_sync_group(
        &mut self,
        session_id: String,
        cx: &mut Context<Self>,
    ) {
        self.shell.set_status(
            match self.sync_input.toggle_active_session_paused(session_id) {
                SyncSessionPauseResult::Paused => "session sync paused",
                SyncSessionPauseResult::Resumed => "session sync resumed",
                SyncSessionPauseResult::NoGroup | SyncSessionPauseResult::NotMember => {
                    "session is not in an active sync group"
                }
            }
            .to_string(),
        );
        cx.notify();
    }

    /// Remove session from its active enabled sync group (Tauri Leave).
    pub(in crate::features) fn leave_active_sync_group(
        &mut self,
        session_id: String,
        cx: &mut Context<Self>,
    ) {
        if !self.sync_input.leave_active_group(&session_id) {
            self.shell
                .set_status("session is not in an active sync group".to_string());
            cx.notify();
            return;
        }
        self.shell.set_status("left sync group".to_string());
        cx.notify();
    }

    /// Disable the active enabled sync group without deleting it (Tauri Close Group).
    pub(in crate::features) fn close_active_sync_group_for_session(
        &mut self,
        session_id: String,
        cx: &mut Context<Self>,
    ) {
        if !self.sync_input.close_active_group(&session_id) {
            self.shell
                .set_status("session is not in an active sync group".to_string());
            cx.notify();
            return;
        }
        self.shell.set_status("sync group closed".to_string());
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use gpui::TestAppContext;

    use super::{GROUP_COLORS, SyncGroupSessionFilter, SyncInputFeatureState};
    use crate::models::SyncInputGroup;

    fn state() -> SyncInputFeatureState {
        let cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let focus = cx.update(|cx| cx.focus_handle());
        SyncInputFeatureState::new(focus)
    }

    fn group(id: &str, name: &str) -> SyncInputGroup {
        SyncInputGroup {
            id: id.to_string(),
            name: name.to_string(),
            color: 0,
            session_ids: Vec::new(),
            paused_session_ids: Vec::new(),
            enabled: true,
        }
    }

    #[test]
    fn group_name_input_updates_only_its_addressed_group() {
        let mut state = state();
        state.groups = vec![group("one", "One"), group("two", "Two")];

        assert!(state.rename_group("two", "Renamed".to_string()));
        assert_eq!(state.groups()[0].name, "One");
        assert_eq!(state.groups()[1].name, "Renamed");
        assert!(!state.rename_group("missing", "Ignored".to_string()));
    }

    #[test]
    fn deleting_and_purging_groups_repairs_the_selected_group() {
        let mut state = state();
        let mut first = group("one", "One");
        first.session_ids = vec!["session-a".to_string()];
        let mut second = group("two", "Two");
        second.session_ids = vec!["session-b".to_string()];
        state.groups = vec![first, second];
        state.selected_id = Some("one".to_string());

        state.purge_session("session-a");
        assert_eq!(state.groups().len(), 1);
        assert_eq!(
            state.selected_group().map(|group| group.id.as_str()),
            Some("two")
        );

        assert!(state.request_delete_selected());
        assert!(state.confirm_delete());
        assert!(state.groups().is_empty());
        assert!(state.selected_group().is_none());
    }

    #[test]
    fn replacing_session_id_updates_membership_and_pause_state_together() {
        let mut state = state();
        let mut sync_group = group("one", "One");
        sync_group.session_ids = vec!["old".to_string(), "peer".to_string()];
        sync_group.paused_session_ids = vec!["old".to_string()];
        state.groups.push(sync_group);

        state.replace_session_id("old", "new");

        assert_eq!(state.groups()[0].session_ids, ["new", "peer"]);
        assert_eq!(state.groups()[0].paused_session_ids, ["new"]);
    }

    #[test]
    fn transfer_splits_group_and_preserves_pause_state_with_conflicting_target_id() {
        let mut source = state();
        let mut original = group("shared", "Shared");
        original.session_ids = vec!["moved".into(), "remaining".into()];
        original.paused_session_ids = vec!["moved".into()];
        source.groups.push(original.clone());
        let mut target = state();
        target.groups.push(group("shared", "Existing"));

        let bundle = source.detach_sessions_for_transfer(&["moved".into()]);
        assert_eq!(source.groups()[0].session_ids, ["remaining"]);
        assert!(source.groups()[0].paused_session_ids.is_empty());
        target.attach_sessions_from_transfer(bundle);
        assert_eq!(target.groups()[0].name, "Existing");
        assert_ne!(target.groups()[1].id, "shared");
        assert_eq!(target.groups()[1].name, "Shared");
        assert_eq!(target.groups()[1].session_ids, ["moved"]);
        assert_eq!(target.groups()[1].paused_session_ids, ["moved"]);
    }

    #[test]
    fn failed_transfer_restores_original_sync_group_membership() {
        let mut source = state();
        let mut original = group("shared", "Shared");
        original.session_ids = vec!["moved".into(), "remaining".into()];
        original.paused_session_ids = vec!["moved".into()];
        source.groups.push(original.clone());

        let bundle = source.detach_sessions_for_transfer(&["moved".into()]);
        source.restore_sessions_after_failed_transfer(bundle);
        assert_eq!(source.groups(), [original]);
    }

    #[test]
    fn peer_selection_honors_pauses_and_broadcast_override() {
        let mut state = state();
        let mut sync_group = group("one", "One");
        sync_group.session_ids = vec![
            "primary".to_string(),
            "peer-a".to_string(),
            "peer-b".to_string(),
        ];
        sync_group.paused_session_ids = vec!["peer-b".to_string()];
        state.groups.push(sync_group);
        let live_ids = ["primary", "peer-a", "peer-b", "outside"]
            .into_iter()
            .map(str::to_string)
            .collect::<HashSet<_>>();

        assert_eq!(
            state.peer_session_ids("primary", &live_ids),
            vec!["peer-a".to_string()]
        );

        assert!(state.toggle_broadcast_to_all());
        assert_eq!(
            state.peer_session_ids("primary", &live_ids),
            vec![
                "outside".to_string(),
                "peer-a".to_string(),
                "peer-b".to_string()
            ]
        );
    }

    #[test]
    fn fan_out_fast_path_requires_an_active_unpaused_source_or_broadcast() {
        let mut state = state();
        assert!(!state.may_fan_out_from("primary"));

        let mut sync_group = group("one", "One");
        sync_group.session_ids = vec!["primary".to_string(), "peer".to_string()];
        sync_group.enabled = false;
        state.groups.push(sync_group);
        assert!(!state.may_fan_out_from("primary"));

        state.groups[0].enabled = true;
        assert!(state.may_fan_out_from("primary"));
        state.groups[0].paused_session_ids = vec!["primary".to_string()];
        assert!(!state.may_fan_out_from("primary"));

        assert!(state.toggle_broadcast_to_all());
        assert!(state.may_fan_out_from("primary"));
        assert!(state.may_fan_out_from("outside"));
    }

    #[test]
    fn unticked_sources_still_broadcast_and_target_exclusions_survive_mode_changes() {
        let mut state = state();
        let live_ids = ["a", "b", "c"]
            .into_iter()
            .map(str::to_string)
            .collect::<HashSet<_>>();
        assert!(!state.input_selection_active());
        assert!(state.toggle_broadcast_to_all());
        assert!(state.input_selection_active());

        let revision = state.revision();
        assert!(state.set_session_receives_input("b", false));
        assert_ne!(state.revision(), revision);
        assert!(!state.set_session_receives_input("b", false));
        assert!(!state.session_receives_input("b"));
        assert!(state.may_fan_out_from("b"));
        assert_eq!(state.peer_session_ids("b", &live_ids), ["a", "c"]);
        assert_eq!(state.peer_session_ids("a", &live_ids), ["c"]);

        assert!(state.set_session_receives_input("b", true));
        assert_eq!(state.peer_session_ids("a", &live_ids), ["b", "c"]);

        state.set_session_receives_input("c", false);
        assert!(!state.toggle_broadcast_to_all());
        assert!(state.toggle_broadcast_to_all());
        assert!(!state.session_receives_input("c"));
    }

    #[test]
    fn target_selection_preserves_group_pause_state_and_can_join_an_enabled_group() {
        let mut state = state();
        let mut disabled = group("off", "Off");
        disabled.enabled = false;
        let mut enabled = group("on", "On");
        enabled.session_ids = vec!["member".to_string()];
        state.groups = vec![disabled, enabled];
        state.selected_id = Some("off".to_string());

        assert!(state.set_session_receives_input("member", false));
        assert!(state.groups()[1].paused_session_ids.is_empty());
        assert!(!state.session_receives_input("member"));
        assert!(state.set_session_receives_input("member", true));
        assert!(state.groups()[1].paused_session_ids.is_empty());

        assert!(state.set_session_receives_input("outsider", false));
        assert!(state.set_session_receives_input("outsider", true));
        assert!(state.groups()[0].session_ids.is_empty());
        assert_eq!(state.groups()[1].session_ids, ["member", "outsider"]);
        assert!(state.session_receives_input("outsider"));

        state.groups[1].enabled = false;
        assert!(!state.input_selection_active());
        assert!(!state.set_session_receives_input("stranger", true));
    }

    #[test]
    fn shared_membership_cannot_bypass_target_veto_and_restore_preserves_pauses() {
        let mut state = state();
        let live = ["one", "two", "three"]
            .map(str::to_string)
            .into_iter()
            .collect();
        for (id, peer) in [("a", "two"), ("b", "three")] {
            let mut g = group(id, id);
            g.session_ids = vec!["one".into(), peer.into()];
            state.groups.push(g);
        }
        let original = state.groups.clone();
        assert!(state.set_session_receives_input("one", false));
        assert!(state.peer_session_ids("two", &live).is_empty());
        assert!(state.peer_session_ids("three", &live).is_empty());
        assert_eq!(state.peer_session_ids("one", &live), ["three", "two"]);
        assert_eq!(state.groups, original);
        state.groups[0].paused_session_ids.push("one".into());
        assert!(state.set_session_receives_input("one", true));
        assert_eq!(state.groups[0].paused_session_ids, ["one"]);
        assert_eq!(state.peer_session_ids("three", &live), ["one"]);
        assert!(state.peer_session_ids("two", &live).is_empty());
        state.toggle_broadcast_to_all();
        assert_eq!(state.peer_session_ids("two", &live), ["one", "three"]);
        state.set_session_receives_input("one", false);
        state.toggle_broadcast_to_all();
        assert!(state.peer_session_ids("three", &live).is_empty());
        state.toggle_broadcast_to_all();
        assert_eq!(state.peer_session_ids("three", &live), ["two"]);
    }

    #[test]
    fn split_target_state_and_revision_follow_batch_selection() {
        let mut state = state();
        state.toggle_broadcast_to_all();
        let ids = vec!["a".into(), "b".into()];
        assert_eq!(state.tab_receive_state(&ids), (true, false));
        state.set_session_receives_input("a", false);
        assert_eq!(state.tab_receive_state(&ids), (false, true));
        state.set_session_receives_input("b", false);
        assert_eq!(state.tab_receive_state(&ids), (false, false));
        for id in &ids {
            state.set_session_receives_input(id, true);
        }
        assert_eq!(state.tab_receive_state(&ids), (true, false));
        let revision = state.revision();
        assert!(!state.set_session_receives_input("b", true));
        assert_eq!(state.revision(), revision);
        assert_eq!(state.tab_receive_state(&[]), (false, false));
    }

    #[test]
    fn exclusions_follow_id_replacement_transfer_rollback_and_close_without_groups() {
        let mut source = state();
        source.set_session_receives_input("old", false);
        source.replace_session_id("old", "new");
        assert!(source.target_allowed("old"));
        assert!(!source.target_allowed("new"));
        let bundle = source.detach_sessions_for_transfer(&["new".into()]);
        assert!(!source.retains_transfer_session("new"));
        source.restore_sessions_after_failed_transfer(bundle);
        assert!(!source.target_allowed("new"));
        let bundle = source.detach_sessions_for_transfer(&["new".into()]);
        let mut target = state();
        target.attach_sessions_from_transfer(bundle);
        assert!(!target.target_allowed("new"));
        target.purge_session("new");
        assert!(!target.retains_transfer_session("new"));
        assert!(target.target_allowed("new"));
    }

    #[test]
    fn adding_or_replacing_sessions_preserves_existing_paused_members() {
        let mut state = state();
        let mut sync_group = group("one", "One");
        sync_group.session_ids = vec!["a".to_string(), "b".to_string()];
        sync_group.paused_session_ids = vec!["a".to_string()];
        state.groups.push(sync_group);
        state.selected_id = Some("one".to_string());

        assert!(state.add_selected_sessions(vec!["b".to_string(), "c".to_string()]));
        assert_eq!(state.groups()[0].session_ids, ["a", "b", "c"]);
        assert_eq!(state.groups()[0].paused_session_ids, ["a"]);

        assert!(state.replace_selected_sessions(vec!["a".to_string(), "d".to_string()]));
        assert_eq!(state.groups()[0].session_ids, ["a", "d"]);
        assert_eq!(state.groups()[0].paused_session_ids, ["a"]);

        assert!(state.replace_selected_sessions(vec!["d".to_string()]));
        assert!(state.groups()[0].paused_session_ids.is_empty());
    }

    #[test]
    fn ensure_selected_group_and_color_cycling_work_as_expected() {
        let mut state = state();
        assert!(state.selected_group().is_none());

        state.ensure_selected_group();
        assert_eq!(state.groups().len(), 1);
        assert_eq!(state.selected_group().unwrap().color, GROUP_COLORS[0]);

        let next_color = state.cycle_selected_color();
        assert_eq!(next_color, Some(GROUP_COLORS[1]));
        assert_eq!(state.selected_group().unwrap().color, GROUP_COLORS[1]);

        state.set_session_filter(SyncGroupSessionFilter::Selected);
        assert_eq!(state.session_filter(), SyncGroupSessionFilter::Selected);
        assert!(state.request_open());
        assert_eq!(state.session_filter(), SyncGroupSessionFilter::All);
    }

    #[test]
    fn window_open_requests_are_deduplicated_and_failures_allow_retry() {
        let mut state = state();
        assert!(state.request_open());
        state.set_search("keep pending search".into());
        assert!(!state.request_open());
        assert_eq!(state.search_draft(), "keep pending search");
        state.window.fail_open();
        assert!(!state.window_is_open_or_pending());
        assert!(state.request_open());
        assert!(state.search_draft().is_empty());
    }

    #[test]
    fn closing_window_clears_transient_state_and_preserves_group_edits() {
        let mut state = state();
        assert!(state.request_open());
        state.create_group();
        let id = state.selected_group().unwrap().id.clone();
        state.rename_group(&id, "Edited group".into());
        state.add_selected_sessions(vec!["session-a".into()]);
        state.toggle_selected_session_paused("session-a".into());
        state.set_search("query".into());
        state.set_session_filter(SyncGroupSessionFilter::Selected);
        assert!(state.request_delete_selected());

        state.close();
        assert!(!state.window_is_open_or_pending());
        assert!(state.search_draft().is_empty());
        assert_eq!(state.session_filter(), SyncGroupSessionFilter::All);
        assert!(state.pending_delete_group().is_none());
        let edited = state.selected_group().unwrap();
        assert_eq!(edited.name, "Edited group");
        assert_eq!(edited.session_ids, ["session-a"]);
        assert_eq!(edited.paused_session_ids, ["session-a"]);
        assert!(state.request_open());
        assert_eq!(state.selected_group().unwrap().id, id);
    }
}
