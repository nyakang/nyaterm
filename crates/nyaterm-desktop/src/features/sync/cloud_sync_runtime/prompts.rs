use rust_i18n::t;

use gpui::{Context, Window};

use crate::features::NyaTermApp;

impl NyaTermApp {
    pub(crate) fn cloud_sync_session_restore_blocked(&self) -> bool {
        cloud_sync_restore_is_blocked(
            self.session.has_live_remote_sessions() || self.remote_desktop.has_active_sessions(),
            self.session.start_has_pending(),
        )
    }

    pub(in crate::features) fn cloud_sync_restore_blocked(&self, cx: &gpui::App) -> bool {
        self.cloud_sync_session_restore_blocked()
            || self
                .desktop_controller
                .as_ref()
                .and_then(gpui::WeakEntity::upgrade)
                .is_some_and(|controller| {
                    controller
                        .read(cx)
                        .cloud_sync_pull_blocked_excluding(self.workspace_id, cx)
                })
    }

    /// The status to show when a restore-style cloud-sync action is refused
    /// because a session is still alive.
    ///
    /// The blocking session can live in another window: every workspace shares
    /// one store, so a pull rewrites the connections that window is using. The
    /// message has to say where the session is — otherwise the user is told to
    /// close "the active session" in a window that has none open.
    pub(in crate::features) fn cloud_sync_restore_block_message(
        &self,
        local_key: &str,
        cx: &gpui::App,
    ) -> Option<String> {
        if !self.cloud_sync_restore_blocked(cx) {
            return None;
        }
        let key = if self.cloud_sync_session_restore_blocked() {
            local_key
        } else {
            "settings.syncCloseActiveSessionInAnotherWindow"
        };
        Some(t!(key).to_string())
    }

    pub(crate) fn cloud_sync_job_running(&self) -> bool {
        self.cloud_sync.job_running()
    }

    pub(in crate::features) fn prompt_provider_cloud_sync_push(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.block_cloud_sync_for_settings_draft(cx) {
            return;
        }
        self.run_provider_cloud_sync_push(false, cx);
    }

    pub(in crate::features) fn prompt_provider_cloud_sync_pull(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.block_cloud_sync_for_settings_draft(cx) {
            return;
        }
        if let Some(message) =
            self.cloud_sync_restore_block_message("settings.syncCloseActiveSessionBeforePull", cx)
        {
            self.shell.set_status(message.clone());
            self.cloud_sync.set_status(message);
            cx.notify();
            return;
        }
        self.run_provider_cloud_sync_pull(false, cx);
    }

    pub(in crate::features) fn prompt_cloud_sync_force_push(
        &mut self,
        provider_action: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.block_cloud_sync_for_settings_draft(cx) {
            return;
        }
        if !self.cloud_sync.prepare_resolution(false) {
            return;
        }
        if provider_action {
            self.run_provider_cloud_sync_push(true, cx);
        } else {
            self.run_local_cloud_sync_push(true, cx);
        }
    }

    pub(in crate::features) fn prompt_cloud_sync_force_pull(
        &mut self,
        provider_action: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.block_cloud_sync_for_settings_draft(cx) {
            return;
        }
        if let Some(message) = self
            .cloud_sync_restore_block_message("settings.syncCloseActiveSessionBeforeForcePull", cx)
        {
            self.shell.set_status(message.clone());
            self.cloud_sync.set_status(message);
            cx.notify();
            return;
        }
        if !self.cloud_sync.prepare_resolution(true) {
            return;
        }
        if provider_action {
            self.run_provider_cloud_sync_pull(true, cx);
        } else {
            self.run_local_cloud_sync_pull(true, cx);
        }
    }

    pub(in crate::features) fn prompt_cloud_sync_recover_current(
        &mut self,
        provider_action: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.block_cloud_sync_for_settings_draft(cx) {
            return;
        }
        if let Some(message) = self
            .cloud_sync_restore_block_message("settings.syncCloseActiveSessionBeforeRecovery", cx)
        {
            self.shell.set_status(message.clone());
            self.cloud_sync.set_status(message);
            cx.notify();
            return;
        }
        self.run_cloud_sync_recovery(provider_action, cx);
    }
}

fn cloud_sync_restore_is_blocked(has_active_session: bool, has_pending_session: bool) -> bool {
    has_active_session || has_pending_session
}

#[cfg(test)]
mod tests {
    use super::cloud_sync_restore_is_blocked;

    #[test]
    fn restore_actions_are_blocked_by_active_or_pending_sessions() {
        assert!(!cloud_sync_restore_is_blocked(false, false));
        assert!(cloud_sync_restore_is_blocked(true, false));
        assert!(cloud_sync_restore_is_blocked(false, true));
        assert!(cloud_sync_restore_is_blocked(true, true));
    }
}
