//! The transfer browser's remote cwd sync, on its own clock.
//!
//! When "Auto CWD" is on for a connection, the browser follows the terminal's working
//! directory by listing the remote cwd on an interval. There is no push from the host,
//! so this is genuinely periodic and stays a poll.
//!
//! It used to ride the shell-wide clock in `features/shell/remote_refresh.rs` alongside
//! the five remote monitor panels. Those took their own clocks in the previous commit,
//! leaving this the only member; it gets a clock of its own here so that file can go.
//!
//! Extracting the Transfers *panel* into an entity is a separate job. This is only the
//! schedule, which is why the armed flag lives on `TransferFeatureState` rather than
//! becoming a `Task` on an entity that does not exist yet.

use std::time::{Duration, Instant};

use gpui::Context;

use crate::features::NyaTermApp;
use crate::features::remote::remote_refresh_due;
use crate::models::NavItem;

/// How often to check whether the cwd sync has come due.
///
/// Matches the interval it services, which is a constant rather than a setting, so
/// there is nothing finer to sample for.
pub(in crate::features) const TRANSFER_CWD_SYNC_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// How stale the browser's cwd may get before it is re-listed.
///
/// Was `TRANSFER_AUTO_SYNC_CWD_INTERVAL_SECONDS` in the event pump's helpers, moved
/// here with the only thing that read it.
const TRANSFER_AUTO_SYNC_CWD_INTERVAL_SECONDS: u32 = 3;

impl NyaTermApp {
    /// Keep the transfer browser's cwd in step while it is open.
    ///
    /// Idempotent. Armed from `render`, because what it depends on -- the browser being
    /// open, and Auto CWD being on for the active connection -- changes alongside a
    /// repaint and has no single event that covers both.
    /// Whether the transfer browser is open at all.
    ///
    /// Only the panel, not Auto CWD or the session: both of those change without a
    /// repaint that would re-arm the clock, so treating them as reasons to retire it
    /// would mean enabling Auto CWD had no effect until something else painted.
    /// `sync_transfer_cwd_if_due` re-checks them every beat instead.
    pub(in crate::features) fn transfer_cwd_sync_needs_polling(&self) -> bool {
        self.current_left_panel() == Some(NavItem::Transfers)
    }

    /// List the remote cwd if it has gone stale.
    ///
    /// The same conditions and the same deferral gate the shell-wide clock applied.
    pub(in crate::features) fn sync_transfer_cwd_if_due(&mut self, cx: &mut Context<Self>) -> bool {
        if self.session.active_file_browser_backend().is_none() || self.remote_refresh_is_deferred()
        {
            return false;
        }
        if !self.transfer_browser_auto_sync_cwd_enabled()
            || self.transfer_sync_cwd_job_running()
            || !remote_refresh_due(
                self.transfer.browser_auto_sync_cwd_last_at(),
                TRANSFER_AUTO_SYNC_CWD_INTERVAL_SECONDS,
            )
        {
            return false;
        }
        self.transfer.mark_browser_auto_sync_cwd(Instant::now());
        self.start_transfer_sync_cwd_job(cx);
        // 定时任务从 App 更新浏览器状态，不会经过面板交互的 with_app。
        self.defer_transfer_panel_snapshot_flush(cx);
        true
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::Duration;

    use gpui::{
        AppContext as _, Entity, IntoElement, ParentElement as _, Render, Styled as _,
        TestAppContext, div,
    };
    use nyaterm_core::{AppRuntime, RuntimeMode};

    use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
    use crate::features::NyaTermApp;
    use crate::models::NavItem;
    use crate::test_support::TestConfigDir;

    fn app(cx: &mut TestAppContext, root: &Path) -> Entity<NyaTermApp> {
        // A uuid rather than a clock reading: these tests run in parallel and
        // Windows' ~15ms clock granularity lets a nanosecond timestamp repeat,
        // which would share one config dir and so one settings database.
        let runtime = AppRuntime::from_parts_for_test(
            RuntimeMode::Portable,
            root.to_path_buf(),
            root.join("config"),
            root.join("logs"),
            root.join("cache"),
            None,
        );
        let stores = UiStoreHandles {
            startup_restore: cx.new(|_| StartupRestoreStore::default()),
            overlays: cx.new(|_| OverlayStore::default()),
        };
        cx.new(|cx| NyaTermApp::new(runtime, stores, cx))
    }

    struct AppHost {
        app: Entity<NyaTermApp>,
    }

    impl Render for AppHost {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            div()
                .size_full()
                .child(self.app.read(cx).transfer_panel.clone())
        }
    }

    fn armed(app: &Entity<NyaTermApp>, cx: &mut gpui::App) -> bool {
        app.read(cx).transfer_panel.read(cx).cwd_clock_is_armed()
    }

    /// Nothing wants the poll, so the panel owns no task.
    #[test]
    fn a_closed_browser_owns_no_clock() {
        let test_dir = TestConfigDir::new("nyaterm-cwd-clock");
        let mut cx = TestAppContext::single();
        let app = app(&mut cx, test_dir.path());
        cx.update_entity(&app, |app, cx| {
            assert!(!app.transfer_cwd_sync_needs_polling());
            app.flush_transfer_panel_snapshot(cx);
        });
        cx.update(|cx| assert!(!armed(&app, cx)));
    }

    /// Opening the browser hands the panel a task, with no paint involved.
    ///
    /// Arming used to happen in `NyaTermApp::render`, which made a repaint the
    /// trigger and left the task owned app-wide. It is the panel's now, so it cannot
    /// outlive the view that wanted it.
    #[test]
    fn opening_the_browser_gives_the_panel_a_clock_without_a_paint() {
        let test_dir = TestConfigDir::new("nyaterm-cwd-clock");
        let mut cx = TestAppContext::single();
        let app = app(&mut cx, test_dir.path());
        cx.update_entity(&app, |app, cx| {
            app.open_or_toggle_panel(NavItem::Transfers, cx);
        });
        cx.update(|cx| {
            assert!(
                armed(&app, cx),
                "opening the transfer browser must give the panel its cwd clock"
            );
        });
    }

    /// Leaving the browser drops it, which cancels the poll.
    #[test]
    fn leaving_the_browser_drops_the_panel_clock() {
        let test_dir = TestConfigDir::new("nyaterm-cwd-clock");
        let mut cx = TestAppContext::single();
        let app = app(&mut cx, test_dir.path());
        cx.update_entity(&app, |app, cx| {
            app.open_or_toggle_panel(NavItem::Transfers, cx);
        });
        cx.update(|cx| assert!(armed(&app, cx)));

        cx.update_entity(&app, |app, cx| {
            app.open_or_toggle_panel(NavItem::Transfers, cx);
            assert!(!app.transfer_cwd_sync_needs_polling());
        });
        cx.update(|cx| {
            assert!(
                !armed(&app, cx),
                "closing the browser must drop the panel's clock, which cancels the poll"
            );
        });
    }

    /// The other half of moving arming off the paint path: repaints of an app that is
    /// not showing the browser must not start polling a remote for its cwd.
    #[test]
    fn unrelated_root_paints_cannot_arm_the_clock() {
        let test_dir = TestConfigDir::new("nyaterm-cwd-clock");
        let mut cx = TestAppContext::single();
        let app = app(&mut cx, test_dir.path());
        cx.update_entity(&app, |app, cx| app.sync_component_theme(cx));

        let host_app = app.clone();
        let (_, cx) = cx.add_window_view(move |_, _| AppHost { app: host_app });
        let cx: &mut gpui::VisualTestContext = cx;
        cx.run_until_parked();

        for _ in 0..5 {
            cx.update(|window, cx| {
                app.update(cx, |_, cx| cx.notify());
                _ = window.draw(cx);
            });
            cx.run_until_parked();
        }

        cx.update(|_, cx| {
            assert!(
                !armed(&app, cx),
                "five repaints with the browser closed must not start a cwd poll"
            );
        });
    }

    /// The poll still hops back to the app: the panel decides when to ask, the app
    /// owns the cwd and every mutation of it.
    #[test]
    fn the_panel_clock_beats_through_the_app() {
        let test_dir = TestConfigDir::new("nyaterm-cwd-clock");
        let mut cx = TestAppContext::single();
        let app = app(&mut cx, test_dir.path());
        cx.update_entity(&app, |app, cx| {
            app.open_or_toggle_panel(NavItem::Transfers, cx);
        });
        cx.update(|cx| assert!(armed(&app, cx)));

        // With no browsable session the app declines every beat, which is the
        // point: the decision is not the panel's to make.
        cx.executor().advance_clock(Duration::from_secs(3));
        cx.run_until_parked();
        cx.update(|cx| {
            assert!(
                armed(&app, cx),
                "the clock must survive beats the app declines"
            );
        });
    }
}
