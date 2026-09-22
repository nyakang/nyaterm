use rust_i18n::t;

use gpui::{Context, Window};

use crate::features::NyaTermApp;
use crate::models::SnapshotPasswordPromptKind;

impl NyaTermApp {
    pub(in crate::features) fn prompt_provider_cloud_sync_push(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.block_cloud_sync_for_settings_draft(cx) {
            return;
        }
        self.start_snapshot_password_prompt(
            SnapshotPasswordPromptKind::CloudProviderPush,
            window,
            cx,
        );
    }

    pub(in crate::features) fn prompt_provider_cloud_sync_pull(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.block_cloud_sync_for_settings_draft(cx) {
            return;
        }
        if cloud_sync_restore_is_blocked(
            self.session.active_id().is_some(),
            self.session.start_has_pending(),
        ) {
            self.shell
                .set_status(t!("settings.syncCloseActiveSessionBeforePull"));
            self.cloud_sync
                .set_status(t!("settings.syncCloseActiveSessionBeforePull"));
            cx.notify();
            return;
        }
        self.start_snapshot_password_prompt(
            SnapshotPasswordPromptKind::CloudProviderPull,
            window,
            cx,
        );
    }

    pub(in crate::features) fn prompt_cloud_sync_force_push(
        &mut self,
        provider_action: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.block_cloud_sync_for_settings_draft(cx) {
            return;
        }
        let kind = if provider_action {
            SnapshotPasswordPromptKind::CloudProviderForcePush
        } else {
            SnapshotPasswordPromptKind::CloudForcePush
        };
        self.start_snapshot_password_prompt(kind, window, cx);
    }

    pub(in crate::features) fn prompt_cloud_sync_force_pull(
        &mut self,
        provider_action: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.block_cloud_sync_for_settings_draft(cx) {
            return;
        }
        if cloud_sync_restore_is_blocked(
            self.session.active_id().is_some(),
            self.session.start_has_pending(),
        ) {
            self.shell
                .set_status(t!("settings.syncCloseActiveSessionBeforeForcePull"));
            self.cloud_sync
                .set_status(t!("settings.syncCloseActiveSessionBeforeForcePull"));
            cx.notify();
            return;
        }
        let kind = if provider_action {
            SnapshotPasswordPromptKind::CloudProviderForcePull
        } else {
            SnapshotPasswordPromptKind::CloudForcePull
        };
        self.start_snapshot_password_prompt(kind, window, cx);
    }

    pub(in crate::features) fn prompt_cloud_sync_recover_current(
        &mut self,
        provider_action: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.block_cloud_sync_for_settings_draft(cx) {
            return;
        }
        if cloud_sync_restore_is_blocked(
            self.session.active_id().is_some(),
            self.session.start_has_pending(),
        ) {
            self.shell
                .set_status(t!("settings.syncCloseActiveSessionBeforeRecovery"));
            self.cloud_sync
                .set_status(t!("settings.syncCloseActiveSessionBeforeRecovery"));
            cx.notify();
            return;
        }
        let kind = if provider_action {
            SnapshotPasswordPromptKind::CloudProviderRecoverCurrent
        } else {
            SnapshotPasswordPromptKind::CloudRecoverCurrent
        };
        self.start_snapshot_password_prompt(kind, window, cx);
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
