use gpui::{Context, Window};

use crate::features::NyaTermApp;

impl NyaTermApp {
    pub(crate) fn cloud_sync_session_restore_blocked(&self) -> bool {
        cloud_sync_restore_is_blocked(
            self.session.has_live_remote_sessions()
                || self
                    .mcp
                    .capabilities
                    .terminal_executions
                    .has_background_operations()
                || self.mcp.capabilities.has_active_requests()
                || self.remote_desktop.blocks_configuration_replacement(),
            self.session.start_is_busy(),
        )
    }

    pub(in crate::features) fn cloud_sync_restore_block_message(
        &mut self,
        local_key: &str,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        let message = self.configuration_replacement_message(local_key, cx);
        if message.is_some() {
            self.retry_configuration_refresh(cx);
        }
        message
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
