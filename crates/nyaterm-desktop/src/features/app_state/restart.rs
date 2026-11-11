use std::process::{Command, Stdio};

use gpui::{Context, Window};
use rust_i18n::t;

use crate::features::NyaTermApp;

/// Handoff token written into the current instance directory before spawning
/// the successor. See `nyaterm_core::RELAUNCH_HANDOFF_ENV`.
const RELAUNCH_HANDOFF_MARKER: &str = "restart.handoff";

impl NyaTermApp {
    pub(crate) fn data_directory_restart_requested(&self) -> bool {
        self.restart_requested
    }

    /// Changes that the ordinary close flow would turn into a dialog instead of
    /// completing the restart. The switch is refused rather than forcing the
    /// process down through unsaved work.
    pub(in crate::features) fn data_directory_restart_blocked(&self) -> bool {
        // These are exactly the gates in `handle_window_close_request_with_count`
        // that would return early, leaving the app running with a queued switch.
        self.settings.theme_designer().submitting
            || self.settings.theme_designer().dirty()
            || self.has_unconfirmed_settings_changes()
            || self.has_unconfirmed_editor_changes()
            || self.notes.has_open_editor_windows()
            || self
                .transfer
                .editor_workspace()
                .is_some_and(|workspace| workspace.tabs.iter().any(|tab| tab.dirty || tab.saving))
    }

    pub(in crate::features) fn notify_data_directory_restart_blocked(
        &self,
        cx: &mut Context<Self>,
    ) {
        self.notify_operation(
            "data-directory-restart-blocked",
            nyaterm_ui::notification::NyaNotificationKind::Warning,
            t!("settings.dataDirectoryBlockedUnsaved").to_string(),
            cx,
        );
    }

    /// Surfaces a failed pending switch that fell back to the previous
    /// directory, so the user is not left guessing after the restart.
    pub(crate) fn notify_data_directory_startup_notice(
        &self,
        notice: String,
        cx: &mut Context<Self>,
    ) {
        self.notify_background_operation(
            "data-directory-startup-notice",
            nyaterm_ui::notification::NyaNotificationKind::Warning,
            format!("{} {}", t!("settings.dataDirectoryNotice"), notice),
            cx,
        );
    }

    pub(in crate::features) fn request_data_directory_restart(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.restart_requested = true;
        // Switching data directories is an explicit quit-and-relaunch action,
        // like installing an update: bypass close-to-tray and the session close
        // confirmation, but keep the settings and editor gates.
        self.handle_window_close_request_with_count(0, window, cx);
    }

    /// Spawns the successor before the workspace workers and store are torn
    /// down, matching the update installer handoff. The child waits for this
    /// process to release its instance lock before it migrates the database.
    pub(crate) fn launch_pending_restart_after_shutdown(
        &mut self,
        _cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if !self.restart_requested {
            return Ok(());
        }
        let executable = std::env::current_exe()
            .map_err(|error| format!("could not resolve the NyaTerm executable: {error}"))?;
        // The marker is the handoff token: the successor waits for the lock in
        // this directory and then removes it, so a stale variable inherited by
        // a later launch cannot make it wait on an unrelated instance.
        let marker = self.runtime.instance_dir().join(RELAUNCH_HANDOFF_MARKER);
        std::fs::write(&marker, [])
            .map_err(|error| format!("could not prepare the NyaTerm restart: {error}"))?;
        Command::new(executable)
            .env(nyaterm_core::RELAUNCH_HANDOFF_ENV, &marker)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| {
                let _ = std::fs::remove_file(&marker);
                format!("could not restart NyaTerm: {error}")
            })?;
        self.restart_requested = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;
    use nyaterm_core::test_support::TestTempDir;

    use crate::features::settings::theme_designer::ThemeIntent;
    use crate::features::test_support::app_with_visible_local_session;
    use crate::test_support::TestAppCleanup;

    #[test]
    fn restart_is_refused_while_settings_or_theme_changes_are_unconfirmed() {
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let root = TestTempDir::new("nyaterm-data-directory-restart");
        let app = app_with_visible_local_session(&mut cx, root.path(), "local");
        app.update(&mut cx, |app, cx| {
            assert!(!app.data_directory_restart_blocked());
            assert!(!app.data_directory_restart_requested());
            app.begin_settings_draft(cx);
            app.set_terminal_scrollback_lines(
                app.settings
                    .summary()
                    .terminal_scrollback_lines
                    .saturating_add(100),
                cx,
            );
            assert!(app.data_directory_restart_blocked());
            app.cancel_settings(cx);
            assert!(!app.data_directory_restart_blocked());
            app.theme_designer_action(ThemeIntent::Copy("github-dark".into()), cx);
            assert!(app.data_directory_restart_blocked());
            app.discard_theme_designer(cx);
            assert!(!app.data_directory_restart_blocked());
        });
    }
}
