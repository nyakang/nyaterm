//! Status text that changes on a clock rather than on an event.
//!
//! Two things in the shell's status area are genuinely time-driven, and both used to
//! ride the runtime tick:
//!
//! * The header's date/time, which `refresh_header_status_clock` re-checked on
//!   **every** tick just to see whether the minute had rolled over. It was the only
//!   thing in the tick body that ran unconditionally, before any cadence or pressure
//!   gate.
//! * The "still connecting" status, which polled a 1s interval while a session start
//!   was pending -- and `start_has_pending()` keeps the tick off the quiet cadence, so
//!   that 1s message was being checked for at 50ms.
//!
//! Both now have a timer scoped to the state that needs them, and stop when it ends.

use std::time::Duration;

use gpui::Context;
use time::OffsetDateTime;

use crate::features::NyaTermApp;
use crate::models::HeaderStatusMode;

use super::event_pump::PENDING_SESSION_STATUS_INTERVAL;

/// How long until the wall clock's next minute boundary.
///
/// The header renders minute precision, so waking on the boundary is both the latest
/// moment that is still correct and the earliest that is worth doing. A minimum of
/// one second keeps a boundary landing exactly on `now` from spinning.
fn duration_to_next_minute(unix_timestamp: i64) -> Duration {
    let into_minute = unix_timestamp.rem_euclid(60);
    Duration::from_secs((60 - into_minute).clamp(1, 60) as u64)
}

impl NyaTermApp {
    /// Keep the header's date/time current, if that is what it is showing.
    ///
    /// Idempotent, and stops itself when the header stops showing a clock, so a
    /// header in any other mode costs nothing.
    pub(in crate::features) fn ensure_header_status_clock(&mut self, cx: &mut Context<Self>) {
        if self.shell.header_status_clock_is_armed() || !self.header_status_clock_should_run() {
            return;
        }
        self.shell.set_header_status_clock_armed(true);
        cx.spawn(async move |this, cx| {
            loop {
                let delay = duration_to_next_minute(OffsetDateTime::now_utc().unix_timestamp());
                cx.background_executor().timer(delay).await;
                let Ok(keep_running) =
                    this.update(cx, |this, cx| this.tick_header_status_clock(cx))
                else {
                    break;
                };
                if !keep_running {
                    break;
                }
            }
        })
        .detach();
    }

    pub(in crate::features) fn header_status_clock_should_run(&self) -> bool {
        self.session_toolbar_context()
            .displays(HeaderStatusMode::DateTime)
    }

    fn tick_header_status_clock(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.header_status_clock_should_run() {
            self.shell.set_header_status_clock_armed(false);
            return false;
        }
        if self.refresh_header_status_clock() {
            cx.notify();
        }
        true
    }

    /// Report a slow connect while one is outstanding.
    ///
    /// Armed when a start is registered rather than polled: the interval only means
    /// anything while `start_pending_status_source` is `Some`, and that is a state
    /// with a definite beginning.
    pub(in crate::features) fn ensure_pending_session_status_clock(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        if self.shell.pending_session_status_clock_is_armed()
            || self.session.start_pending_status_source().is_none()
        {
            return;
        }
        self.shell.set_pending_session_status_clock_armed(true);
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(PENDING_SESSION_STATUS_INTERVAL)
                    .await;
                let Ok(keep_running) =
                    this.update(cx, |this, cx| this.tick_pending_session_status(cx))
                else {
                    break;
                };
                if !keep_running {
                    break;
                }
            }
        })
        .detach();
    }

    fn tick_pending_session_status(&mut self, cx: &mut Context<Self>) -> bool {
        // Ticking at the interval rather than sleeping straight to the 15s mark: an
        // auth prompt makes the message due early, and that arrives as an event this
        // clock does not see.
        if self.drive_pending_session_status() {
            cx.notify();
        }
        if self.session.start_pending_status_source().is_some() {
            return true;
        }
        self.shell.set_pending_session_status_clock_armed(false);
        false
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::Duration;

    use gpui::{AppContext as _, TestAppContext};
    use nyaterm_core::{AppRuntime, RuntimeMode};

    use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
    use crate::features::NyaTermApp;
    use crate::models::HeaderStatusMode;
    use crate::test_support::TestConfigDir;

    use super::duration_to_next_minute;

    fn app_with_header_mode(
        cx: &mut TestAppContext,
        root: &Path,
        mode: HeaderStatusMode,
    ) -> gpui::Entity<NyaTermApp> {
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
        let app = cx.new(|cx| NyaTermApp::new(runtime, stores, cx));
        cx.update_entity(&app, |app, _| {
            let mut summary = app.settings.summary().clone();
            summary.ui_header_status_visible = true;
            summary.ui_header_status_mode = mode.persistence_id().to_string();
            app.settings.replace_summary(summary);
        });
        app
    }

    #[test]
    fn the_header_clock_wakes_on_the_minute_boundary() {
        // A timestamp exactly on a boundary has a whole minute to wait.
        assert_eq!(duration_to_next_minute(60), Duration::from_secs(60));
        assert_eq!(duration_to_next_minute(61), Duration::from_secs(59));
        assert_eq!(duration_to_next_minute(119), Duration::from_secs(1));
        // The last second of a minute must still wait a second, never zero, or the
        // clock spins instead of sleeping.
        assert_eq!(duration_to_next_minute(120), Duration::from_secs(60));
    }

    #[test]
    fn the_header_clock_handles_pre_epoch_timestamps() {
        // `rem_euclid` rather than `%`, so a negative timestamp still yields a delay
        // inside the interval instead of something larger than a minute.
        for timestamp in [-1, -59, -60, -61] {
            let delay = duration_to_next_minute(timestamp);
            assert!(
                delay >= Duration::from_secs(1) && delay <= Duration::from_secs(60),
                "timestamp {timestamp} produced {delay:?}"
            );
        }
    }

    /// A header that is not showing a clock must not be running one.
    ///
    /// This is the whole point of scoping the timer: `refresh_header_status_clock`
    /// used to be re-checked on every tick regardless of the header's mode.
    #[test]
    fn no_clock_runs_for_a_header_that_shows_no_time() {
        let test_dir = TestConfigDir::new("nyaterm-status-clocks");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_header_mode(&mut cx, test_dir.path(), HeaderStatusMode::Session);
        cx.update_entity(&app, |app, cx| {
            assert!(!app.header_status_clock_should_run());
            app.ensure_header_status_clock(cx);
            assert!(
                !app.shell.header_status_clock_is_armed(),
                "a session-mode header has no minute to track"
            );
        });
    }

    /// Switching the header off stops the clock rather than leaving it waking hourly
    /// for nothing.
    #[test]
    fn the_header_clock_stops_when_the_header_stops_showing_time() {
        let test_dir = TestConfigDir::new("nyaterm-status-clocks");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_header_mode(&mut cx, test_dir.path(), HeaderStatusMode::DateTime);
        cx.update_entity(&app, |app, cx| {
            app.ensure_header_status_clock(cx);
            assert!(app.shell.header_status_clock_is_armed());

            let mut summary = app.settings.summary().clone();
            summary.ui_header_status_visible = false;
            app.settings.replace_summary(summary);
        });

        // One boundary later the clock notices and retires itself.
        cx.executor().advance_clock(Duration::from_secs(60));
        cx.run_until_parked();
        cx.update_entity(&app, |app, _| {
            assert!(
                !app.shell.header_status_clock_is_armed(),
                "the clock must retire itself, not keep waking every minute"
            );
        });
    }

    /// Nothing pending means no 1s wake, which is the state the app spends most of
    /// its life in.
    #[test]
    fn no_pending_status_clock_runs_without_a_pending_start() {
        let test_dir = TestConfigDir::new("nyaterm-status-clocks");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_header_mode(&mut cx, test_dir.path(), HeaderStatusMode::Session);
        cx.update_entity(&app, |app, cx| {
            assert!(app.session.start_pending_status_source().is_none());
            app.ensure_pending_session_status_clock(cx);
            assert!(!app.shell.pending_session_status_clock_is_armed());
        });
    }
}
