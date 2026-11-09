use crate::entities::startup_restore::{StartupRestoreFocusHandoff, StartupRestoreFocusSnapshot};
use crate::features::NyaTermApp;
use gpui::{Context, Window};

impl NyaTermApp {
    pub(in crate::features) fn observe_startup_restore_focus_activation(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.ensure_startup_restore_focus_handoff(window, cx);
            }
        })
        .detach();
    }

    fn startup_restore_focus_is_blocked(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        !self.settings.summary().startup_restore
            || self.root_overlay_visibility(cx).blocks_workspace_focus()
            || self.floating_panel_dismissal_is_blocked(window, cx)
            || self.has_pending_focus_request()
            || window.focused(cx).is_some_and(|focus| {
                focus != *self.terminal.input_focus() && focus != *self.remote_desktop.focus()
            })
    }

    fn startup_restore_focus_is_ready(&self, cx: &Context<Self>) -> bool {
        self.session.restore_is_complete()
            && self.stores.startup_restore.read(cx).queue_empty()
            && !self.session.start_is_busy()
            && self.remote_desktop.restore_pending.is_none()
            && self.shell.workspace_layout_restore_is_settled()
            && self.shell.terminal_windows_restore_is_complete()
    }

    fn startup_restore_focus_snapshot(
        &self,
        window: &Window,
        cx: &Context<Self>,
    ) -> Option<StartupRestoreFocusSnapshot> {
        if self.session.start_has_active_failed() {
            return None;
        }
        Some(StartupRestoreFocusSnapshot {
            workspace_tab: self.active_workspace_tab()?,
            active_session: self.session.active_id_owned(),
            terminal_group: self.current_terminal_group(),
            focus: window.focused(cx),
        })
    }

    pub(in crate::features) fn ensure_startup_restore_focus_handoff(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(
            self.stores.startup_restore.read(cx).focus_handoff(),
            StartupRestoreFocusHandoff::Waiting(_)
        ) {
            return;
        }
        if self.startup_restore_focus_is_blocked(window, cx) {
            self.stores
                .startup_restore
                .update(cx, |store, _| store.finish_focus_handoff());
            return;
        }
        if !self.startup_restore_focus_is_ready(cx) {
            return;
        }
        let Some(snapshot) = self.startup_restore_focus_snapshot(window, cx) else {
            self.stores
                .startup_restore
                .update(cx, |store, _| store.finish_focus_handoff());
            return;
        };
        if !self
            .stores
            .startup_restore
            .update(cx, |store, _| store.prepare_focus_handoff(snapshot))
            || !window.is_window_active()
        {
            return;
        }
        let snapshot = self
            .stores
            .startup_restore
            .update(cx, |store, _| store.schedule_focus_handoff());
        if let Some(snapshot) = snapshot {
            // The restored pane must be mounted before its focus handle can
            // receive keyboard input. Rendering only schedules this handoff.
            cx.on_next_frame(window, move |this, window, cx| {
                this.apply_startup_restore_focus_handoff(snapshot, window, cx);
            });
        }
    }

    fn apply_startup_restore_focus_handoff(
        &mut self,
        snapshot: StartupRestoreFocusSnapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(
            self.stores.startup_restore.read(cx).focus_handoff(),
            StartupRestoreFocusHandoff::Scheduled(current) if current == &snapshot
        ) {
            return;
        }
        if self.startup_restore_focus_is_blocked(window, cx)
            || self.startup_restore_focus_snapshot(window, cx).as_ref() != Some(&snapshot)
        {
            self.stores
                .startup_restore
                .update(cx, |store, _| store.finish_focus_handoff());
            return;
        }
        if !self.startup_restore_focus_is_ready(cx) || !window.is_window_active() {
            self.stores
                .startup_restore
                .update(cx, |store, _| store.defer_focus_handoff());
            return;
        }
        self.stores
            .startup_restore
            .update(cx, |store, _| store.finish_focus_handoff());
        self.focus_active_workspace_surface(window, cx);
    }
}

#[cfg(test)]
mod tests;
