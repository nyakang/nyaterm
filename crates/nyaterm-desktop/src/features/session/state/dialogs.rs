//! session dialogs state and transitions.

use super::SessionFeatureState;
use super::start::{DEFAULT_DUPLICATE_STARTUP_DELAY_MS, StartTabActionSnapshot};
use crate::features::shell::tab_mouse::TabActionTarget;
use crate::models::{StartupCommandAction, StartupCommandRequest, TabActionsSubmenu};
use crate::temporary_ssh_link::TemporaryLinkProtocol;
use gpui::{FocusHandle, ScrollHandle, point, px};

/// Session-scoped overlays, confirmations and editing dialogs.
pub(in crate::features::session) struct SessionDialogState {
    pub(super) tab_actions_target: Option<TabActionTarget>,
    pub(super) tab_actions_start_snapshot: Option<StartTabActionSnapshot>,
    pub(super) tab_actions_anchor: Option<(f32, f32)>,
    pub(super) tab_actions_submenu: Option<TabActionsSubmenu>,
    pub(super) tab_actions_focus: FocusHandle,
    pub(super) tab_actions_restore_focus: Option<FocusHandle>,
    pub(super) pending_quit_after_close_all: bool,
    pub(super) pending_window_quit: bool,
    pub(super) rename_target: Option<TabActionTarget>,
    pub(super) rename_dialog_handle: Option<nyaterm_ui::NyaDialogHandle>,
    pub(super) rename_draft: String,
    pub(super) color_picker_open: bool,
    pub(super) color_picker_focus: FocusHandle,
    pub(super) session_info_open: bool,
    pub(super) session_info_focus: FocusHandle,
    pub(super) session_info_offset: (f32, f32),
    pub(super) session_info_drag: Option<SessionInfoDrag>,
    pub(super) session_info_scroll: ScrollHandle,
    pub(super) session_info_dialog_height: f32,
    pub(super) startup_command_open: bool,
    pub(super) startup_command_action: StartupCommandAction,
    pub(super) startup_command_draft: String,
    pub(super) startup_command_delay_ms: u64,
    pub(super) temporary_ssh_link_open: bool,
    pub(super) temporary_link_protocol: TemporaryLinkProtocol,
    pub(super) temporary_ssh_link_draft: String,
    pub(super) temporary_serial_port_name: String,
    pub(super) temporary_serial_baud_rate: String,
    pub(super) temporary_ssh_link_error: Option<&'static str>,
}

pub(in crate::features) enum RenameSessionSubmission {
    Inactive,
    Empty,
    Ready {
        target: TabActionTarget,
        name: String,
    },
}

/// Pointer and offset at the moment the session-info header was grabbed, so
/// the dialog follows the pointer instead of jumping to it.
pub(super) struct SessionInfoDrag {
    pub(super) pointer: (f32, f32),
    pub(super) offset: (f32, f32),
}

impl SessionDialogState {
    pub(in crate::features) fn tab_actions_session_id(&self) -> Option<&str> {
        match self.tab_actions_target.as_ref()? {
            TabActionTarget::Session(id) => Some(id),
            TabActionTarget::Start { .. } => None,
        }
    }

    pub(in crate::features) fn tab_actions_anchor(&self) -> Option<(f32, f32)> {
        self.tab_actions_anchor
    }

    pub(in crate::features) fn tab_actions_submenu(&self) -> Option<TabActionsSubmenu> {
        self.tab_actions_submenu
    }

    pub(in crate::features) fn tab_actions_focus(&self) -> &FocusHandle {
        &self.tab_actions_focus
    }

    pub(in crate::features) fn open_tab_actions(
        &mut self,
        session_id: String,
        anchor: Option<(f32, f32)>,
    ) {
        self.tab_actions_target = Some(TabActionTarget::Session(session_id));
        self.tab_actions_start_snapshot = None;
        self.tab_actions_restore_focus = None;
        self.tab_actions_anchor = anchor;
        self.tab_actions_submenu = None;
    }

    pub(in crate::features) fn open_start_tab_actions(
        &mut self,
        target: TabActionTarget,
        snapshot: StartTabActionSnapshot,
        anchor: (f32, f32),
        restore_focus: Option<FocusHandle>,
    ) {
        self.tab_actions_restore_focus = restore_focus;
        self.tab_actions_target = Some(target);
        self.tab_actions_start_snapshot = Some(snapshot);
        self.tab_actions_anchor = Some(anchor);
        self.tab_actions_submenu = None;
    }

    pub(in crate::features) fn take_tab_actions_restore_focus(&mut self) -> Option<FocusHandle> {
        self.tab_actions_restore_focus.take()
    }

    pub(in crate::features) fn close_tab_actions(&mut self) {
        self.tab_actions_target = None;
        self.tab_actions_start_snapshot = None;
        self.tab_actions_restore_focus = None;
        self.tab_actions_anchor = None;
        self.tab_actions_submenu = None;
    }

    pub(in crate::features) fn select_tab_actions_submenu(
        &mut self,
        submenu: TabActionsSubmenu,
    ) -> bool {
        if self.tab_actions_submenu == Some(submenu) {
            return false;
        }
        self.tab_actions_submenu = Some(submenu);
        true
    }

    pub(in crate::features) fn should_quit_after_close_all(&self) -> bool {
        self.pending_quit_after_close_all
    }

    pub(in crate::features) fn request_quit_after_close_all(&mut self) {
        self.pending_quit_after_close_all = true;
    }

    pub(in crate::features) fn open_close_all_sessions_confirm(&mut self) {
        self.close_tab_actions();
    }

    pub(in crate::features) fn cancel_close_all_sessions_confirm(&mut self) {
        self.pending_quit_after_close_all = false;
        self.pending_window_quit = false;
    }

    pub(in crate::features) fn take_close_all_sessions_confirm(&mut self) -> bool {
        let quit_after = self.pending_quit_after_close_all;
        self.pending_quit_after_close_all = false;
        self.pending_window_quit = false;
        quit_after
    }

    pub(in crate::features) fn rename_dialog_handle(&self) -> Option<&nyaterm_ui::NyaDialogHandle> {
        self.rename_dialog_handle.as_ref()
    }

    pub(in crate::features) fn set_rename_dialog_handle(
        &mut self,
        handle: nyaterm_ui::NyaDialogHandle,
    ) {
        self.rename_dialog_handle = Some(handle);
    }

    pub(in crate::features) fn rename_target(&self) -> Option<&TabActionTarget> {
        self.rename_target.as_ref()
    }

    pub(in crate::features) fn rename_draft(&self) -> &str {
        &self.rename_draft
    }

    #[cfg(test)]
    pub(in crate::features) fn open_rename(&mut self, session_id: String, current_name: &str) {
        self.open_tab_rename(TabActionTarget::Session(session_id), current_name);
    }

    pub(in crate::features) fn open_tab_rename(
        &mut self,
        target: TabActionTarget,
        current_name: &str,
    ) {
        self.rename_target = Some(target);
        self.rename_dialog_handle = None;
        self.rename_draft = current_name.chars().take(64).collect();
    }

    pub(in crate::features) fn cancel_rename(&mut self) {
        self.rename_target = None;
        self.rename_dialog_handle = None;
        self.rename_draft.clear();
    }

    pub(in crate::features) fn take_rename_submission(&mut self) -> RenameSessionSubmission {
        let Some(target) = self.rename_target.take() else {
            return RenameSessionSubmission::Inactive;
        };
        let name = self
            .rename_draft
            .trim()
            .chars()
            .take(64)
            .collect::<String>();
        self.rename_draft.clear();
        if name.is_empty() {
            self.rename_target = Some(target);
            return RenameSessionSubmission::Empty;
        }
        self.rename_dialog_handle = None;
        RenameSessionSubmission::Ready { target, name }
    }

    pub(in crate::features) fn color_picker_is_open(&self) -> bool {
        self.color_picker_open
    }

    pub(in crate::features) fn color_picker_focus(&self) -> &FocusHandle {
        &self.color_picker_focus
    }

    pub(in crate::features) fn close_color_picker(&mut self) {
        self.color_picker_open = false;
    }

    pub(in crate::features) fn session_info_is_open(&self) -> bool {
        self.session_info_open
    }

    pub(in crate::features) fn session_info_focus(&self) -> &FocusHandle {
        &self.session_info_focus
    }

    pub(in crate::features) fn session_info_offset(&self) -> (f32, f32) {
        self.session_info_offset
    }

    pub(in crate::features) fn session_info_drag_active(&self) -> bool {
        self.session_info_drag.is_some()
    }

    pub(in crate::features) fn session_info_scroll(&self) -> &ScrollHandle {
        &self.session_info_scroll
    }

    pub(in crate::features) fn session_info_dialog_height(&self) -> f32 {
        self.session_info_dialog_height
    }

    /// Measured during prepaint. Never notifies: drag limits are read at event
    /// time, so a new measurement only needs to be stored, not painted.
    pub(in crate::features) fn set_session_info_dialog_height(&mut self, height: f32) {
        self.session_info_dialog_height = height.max(0.);
    }

    pub(in crate::features) fn begin_session_info_drag(&mut self, pointer: (f32, f32)) {
        self.session_info_drag = Some(SessionInfoDrag {
            pointer,
            offset: self.session_info_offset,
        });
    }

    pub(in crate::features) fn drag_session_info(
        &mut self,
        pointer: (f32, f32),
        limits: (f32, f32),
    ) -> bool {
        let Some(drag) = &self.session_info_drag else {
            return false;
        };
        let next = (
            (drag.offset.0 + pointer.0 - drag.pointer.0).clamp(-limits.0, limits.0),
            (drag.offset.1 + pointer.1 - drag.pointer.1).clamp(-limits.1, limits.1),
        );
        if next == self.session_info_offset {
            return false;
        }
        self.session_info_offset = next;
        true
    }

    pub(in crate::features) fn end_session_info_drag(&mut self) {
        self.session_info_drag = None;
    }

    pub(in crate::features) fn open_session_info(&mut self) {
        self.session_info_open = true;
        self.session_info_offset = (0., 0.);
        self.session_info_drag = None;
        self.session_info_dialog_height = 0.;
        self.session_info_scroll.set_offset(point(px(0.), px(0.)));
    }

    pub(in crate::features) fn close_session_info(&mut self) {
        self.session_info_open = false;
        self.session_info_offset = (0., 0.);
        self.session_info_drag = None;
        self.session_info_dialog_height = 0.;
    }

    pub(in crate::features) fn startup_command_draft(&self) -> &str {
        &self.startup_command_draft
    }

    pub(in crate::features) fn startup_command_delay_ms(&self) -> u64 {
        self.startup_command_delay_ms
    }

    pub(in crate::features) fn open_startup_command(
        &mut self,
        action: StartupCommandAction,
        delay_ms: u64,
    ) {
        self.startup_command_open = true;
        self.startup_command_action = action;
        self.startup_command_draft.clear();
        self.startup_command_delay_ms = delay_ms.min(60_000);
    }

    pub(in crate::features) fn cancel_startup_command(&mut self) -> StartupCommandAction {
        let action = self.startup_command_action;
        self.startup_command_open = false;
        self.startup_command_action = StartupCommandAction::Duplicate;
        self.startup_command_draft.clear();
        self.startup_command_delay_ms = DEFAULT_DUPLICATE_STARTUP_DELAY_MS;
        action
    }

    pub(in crate::features) fn take_startup_command(
        &mut self,
    ) -> Option<(StartupCommandAction, StartupCommandRequest)> {
        let command = self.startup_command_draft.trim().to_string();
        if command.is_empty() {
            return None;
        }
        let request = StartupCommandRequest {
            command,
            delay_ms: self.startup_command_delay_ms.min(60_000),
        };
        let action = self.startup_command_action;
        self.startup_command_open = false;
        self.startup_command_action = StartupCommandAction::Duplicate;
        self.startup_command_draft.clear();
        Some((action, request))
    }

    pub(in crate::features) fn set_startup_command_delay(&mut self, delay_ms: u64) {
        self.startup_command_delay_ms = delay_ms.min(60_000);
    }

    pub(in crate::features) fn reset_startup_command_delay(&mut self) {
        self.startup_command_delay_ms = 0;
    }

    pub(in crate::features) fn temporary_ssh_link_draft(&self) -> &str {
        &self.temporary_ssh_link_draft
    }

    pub(in crate::features) fn temporary_link_protocol(&self) -> TemporaryLinkProtocol {
        self.temporary_link_protocol
    }

    pub(in crate::features) fn temporary_serial_port_name(&self) -> &str {
        &self.temporary_serial_port_name
    }

    pub(in crate::features) fn temporary_serial_baud_rate(&self) -> &str {
        &self.temporary_serial_baud_rate
    }

    pub(in crate::features) fn temporary_ssh_link_error(&self) -> Option<&'static str> {
        self.temporary_ssh_link_error
    }

    pub(in crate::features) fn open_temporary_ssh_link(&mut self) {
        self.temporary_ssh_link_open = true;
        self.temporary_ssh_link_error = None;
    }

    pub(in crate::features) fn close_temporary_ssh_link(&mut self) {
        self.temporary_ssh_link_open = false;
        self.temporary_link_protocol = TemporaryLinkProtocol::Ssh;
        self.temporary_ssh_link_draft.clear();
        self.temporary_serial_port_name.clear();
        self.temporary_serial_baud_rate = "115200".to_string();
        self.temporary_ssh_link_error = None;
    }

    pub(in crate::features) fn set_temporary_link_protocol(
        &mut self,
        protocol: TemporaryLinkProtocol,
    ) {
        self.temporary_link_protocol = protocol;
        self.temporary_ssh_link_error = None;
    }

    pub(in crate::features) fn reject_temporary_ssh_link(&mut self, error: &'static str) {
        self.temporary_ssh_link_error = Some(error);
    }

    pub(in crate::features) fn apply_temporary_ssh_link(&mut self, text: String) {
        self.temporary_ssh_link_draft = text;
        self.temporary_ssh_link_error = None;
    }

    pub(in crate::features) fn apply_temporary_serial_port_name(&mut self, text: String) {
        self.temporary_serial_port_name = text;
        self.temporary_ssh_link_error = None;
    }

    pub(in crate::features) fn apply_temporary_serial_baud_rate(&mut self, text: String) {
        self.temporary_serial_baud_rate = text.chars().filter(|ch| ch.is_ascii_digit()).collect();
        self.temporary_ssh_link_error = None;
    }

    pub(in crate::features) fn apply_text_input(&mut self, field: &str, text: String) -> bool {
        match field {
            "rename" => self.rename_draft = text,
            "startup-command" => self.startup_command_draft = text,
            _ => return false,
        }
        true
    }
}

impl SessionFeatureState {
    pub(in crate::features) fn dialog_tab_actions_session_id(&self) -> Option<&str> {
        self.dialogs.tab_actions_session_id()
    }
    pub(in crate::features) fn dialog_tab_actions_target(&self) -> Option<&TabActionTarget> {
        self.dialogs.tab_actions_target.as_ref()
    }
    pub(in crate::features) fn dialog_tab_actions_start_snapshot(
        &self,
    ) -> Option<&StartTabActionSnapshot> {
        self.dialogs.tab_actions_start_snapshot.as_ref()
    }
    pub(in crate::features) fn dialog_tab_actions_anchor(&self) -> Option<(f32, f32)> {
        self.dialogs.tab_actions_anchor()
    }
    pub(in crate::features) fn dialog_tab_actions_submenu(&self) -> Option<TabActionsSubmenu> {
        self.dialogs.tab_actions_submenu()
    }
    pub(in crate::features) fn dialog_tab_actions_focus(&self) -> &FocusHandle {
        self.dialogs.tab_actions_focus()
    }
    pub(in crate::features) fn dialog_close_tab_actions(&mut self) {
        self.dialogs.close_tab_actions();
    }
    pub(in crate::features) fn dialog_select_tab_actions_submenu(
        &mut self,
        submenu: TabActionsSubmenu,
    ) -> bool {
        self.dialogs.select_tab_actions_submenu(submenu)
    }
    pub(in crate::features) fn dialog_should_quit_after_close_all(&self) -> bool {
        self.dialogs.should_quit_after_close_all()
    }
    pub(in crate::features) fn dialog_request_quit_after_close_all(&mut self) {
        self.dialogs.request_quit_after_close_all();
    }
    pub(in crate::features) fn dialog_open_close_all_sessions_confirm(&mut self) {
        self.dialogs.open_close_all_sessions_confirm();
    }
    pub(in crate::features) fn dialog_cancel_close_all_sessions_confirm(&mut self) {
        self.dialogs.cancel_close_all_sessions_confirm();
    }
    pub(in crate::features) fn dialog_take_close_all_sessions_confirm(&mut self) -> bool {
        self.dialogs.take_close_all_sessions_confirm()
    }
    pub(in crate::features) fn dialog_rename_draft(&self) -> &str {
        self.dialogs.rename_draft()
    }
    pub(in crate::features) fn dialog_color_picker_is_open(&self) -> bool {
        self.dialogs.color_picker_is_open()
    }
    pub(in crate::features) fn dialog_color_picker_focus(&self) -> &FocusHandle {
        self.dialogs.color_picker_focus()
    }
    pub(in crate::features) fn dialog_session_info_is_open(&self) -> bool {
        self.dialogs.session_info_is_open()
    }
    pub(in crate::features) fn dialog_session_info_focus(&self) -> &FocusHandle {
        self.dialogs.session_info_focus()
    }
    pub(in crate::features) fn dialog_session_info_offset(&self) -> (f32, f32) {
        self.dialogs.session_info_offset()
    }
    pub(in crate::features) fn dialog_session_info_drag_active(&self) -> bool {
        self.dialogs.session_info_drag_active()
    }
    pub(in crate::features) fn dialog_session_info_scroll(&self) -> ScrollHandle {
        self.dialogs.session_info_scroll().clone()
    }
    pub(in crate::features) fn dialog_session_info_dialog_height(&self) -> f32 {
        self.dialogs.session_info_dialog_height()
    }
    pub(in crate::features) fn dialog_set_session_info_dialog_height(&mut self, height: f32) {
        self.dialogs.set_session_info_dialog_height(height);
    }
    pub(in crate::features) fn dialog_begin_session_info_drag(&mut self, pointer: (f32, f32)) {
        self.dialogs.begin_session_info_drag(pointer);
    }
    pub(in crate::features) fn dialog_drag_session_info(
        &mut self,
        pointer: (f32, f32),
        limits: (f32, f32),
    ) -> bool {
        self.dialogs.drag_session_info(pointer, limits)
    }
    pub(in crate::features) fn dialog_constrain_session_info(&mut self, limits: (f32, f32)) {
        let offset = &mut self.dialogs.session_info_offset;
        let next = (
            offset.0.clamp(-limits.0, limits.0),
            offset.1.clamp(-limits.1, limits.1),
        );
        if *offset != next {
            *offset = next;
            self.dialogs.session_info_drag = None;
        }
    }
    pub(in crate::features) fn dialog_end_session_info_drag(&mut self) {
        self.dialogs.end_session_info_drag();
    }
    pub(in crate::features) fn dialog_startup_command_draft(&self) -> &str {
        self.dialogs.startup_command_draft()
    }
    pub(in crate::features) fn dialog_startup_command_delay_ms(&self) -> u64 {
        self.dialogs.startup_command_delay_ms()
    }
    pub(in crate::features) fn dialog_reset_startup_command_delay(&mut self) {
        self.dialogs.reset_startup_command_delay();
    }
    pub(in crate::features) fn dialog_temporary_ssh_link_draft(&self) -> &str {
        self.dialogs.temporary_ssh_link_draft()
    }
    pub(in crate::features) fn dialog_temporary_link_protocol(&self) -> TemporaryLinkProtocol {
        self.dialogs.temporary_link_protocol()
    }
    pub(in crate::features) fn dialog_temporary_serial_port_name(&self) -> &str {
        self.dialogs.temporary_serial_port_name()
    }
    pub(in crate::features) fn dialog_temporary_serial_baud_rate(&self) -> &str {
        self.dialogs.temporary_serial_baud_rate()
    }
    pub(in crate::features) fn dialog_temporary_ssh_link_error(&self) -> Option<&'static str> {
        self.dialogs.temporary_ssh_link_error()
    }
    pub(in crate::features) fn clear_start_tab_placements(&mut self) {
        // Placement reservations are temporary; an open rename must retain the
        // formal session identity before the last reservation is released.
        if let Some(target @ TabActionTarget::Start { .. }) = self.dialogs.rename_target.as_ref()
            && let Some(id) = self.resolved_tab_action_session(target)
        {
            self.dialogs.rename_target = Some(TabActionTarget::Session(id));
        }
        self.start_tab_placements.clear();
        self.start.placements.clear();
    }
}
