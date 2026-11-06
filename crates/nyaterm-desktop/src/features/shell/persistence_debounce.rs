//! Debounced durable writes for open tabs, window layout and UI layout.
//!
//! These three used to be dirty flags drained by the runtime tick's idle plane,
//! which made them a recurring defect: the flag's only driver was that plane, the
//! plane only ran off the quiet cadence, and so every flag had to be named in
//! `runtime_quiet_tick_allowed` or silently never flush. That went wrong three
//! times, most recently for `ui_layout_persist_pending` (`4644195a`), where a
//! header-status change was applied to state and never written.
//!
//! Now a single task owns them. A mark signals a wake, the task waits out a
//! coalescing window, and then writes whatever is still dirty -- so a new flag
//! cannot be added without a writer, and no central predicate needs to know they
//! exist.

use std::time::{Duration, Instant};

use futures::{FutureExt as _, StreamExt as _};
use gpui::Context;

use crate::features::NyaTermApp;

/// How long to collect marks before writing.
///
/// Nothing observes these writes until the next launch, so the window is chosen to
/// coalesce rather than to be prompt: a tab reorder, a split resize and a panel drag
/// in quick succession should cost one write, not three. The runtime tick's idle
/// plane effectively used one tick (50ms once calm).
const SHELL_PERSISTENCE_DEBOUNCE: Duration = Duration::from_millis(500);

/// How long to wait before re-checking when the write had to be held off.
const SHELL_PERSISTENCE_RETRY: Duration = Duration::from_millis(500);

/// Whether a durable shell write may run right now.
///
/// These writes open the config database, so they wait for calm: the idle plane got
/// this from `runtime_idle_plane_allowed`'s `demote_idle` term, and losing it would
/// mean opening the DB during the first frames after a connect, or on every frame of
/// a window drag. Keeping it also turns a drag into one write once it settles rather
/// than one per frame.
fn shell_persistence_write_is_allowed(
    output_pressure: bool,
    geometry_busy: bool,
    connect_settle: bool,
) -> bool {
    !output_pressure && !geometry_busy && !connect_settle
}

impl NyaTermApp {
    /// Write shell persistence on a debounce instead of on the runtime tick.
    ///
    /// Started once at window open. The task parks on the wake when nothing is
    /// dirty, so an app that never changes its layout costs nothing.
    pub(in crate::features) fn start_shell_persistence_debounce(&mut self, cx: &mut Context<Self>) {
        let Some(mut wake_rx) = self.shell.take_persist_wake_receiver() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            loop {
                // Arm before checking the flags, or a mark landing between the check
                // and the arm leaves a dirty flag with nothing scheduled to write it.
                let Ok(pending) = this.update(cx, |this, _| {
                    this.shell.arm_persist_wake();
                    this.shell.has_pending_persistence()
                }) else {
                    break;
                };
                if !pending {
                    if wake_rx.next().await.is_none() {
                        break;
                    }
                    continue;
                }

                // Collect further marks, but take an early wake so the window starts
                // from the first mark rather than drifting with each one.
                let mut timer = cx
                    .background_executor()
                    .timer(SHELL_PERSISTENCE_DEBOUNCE)
                    .fuse();
                futures::select_biased! {
                    _ = timer => {}
                    wake = wake_rx.next() => {
                        if wake.is_none() {
                            break;
                        }
                    }
                }

                let Ok(retry_in) =
                    this.update(cx, |this, cx| this.flush_shell_persistence_debounce(cx))
                else {
                    break;
                };
                if let Some(delay) = retry_in {
                    cx.background_executor().timer(delay).await;
                }
            }
        })
        .detach();
    }

    /// Write what is dirty, or report how long to wait before trying again.
    fn flush_shell_persistence_debounce(&mut self, cx: &mut Context<Self>) -> Option<Duration> {
        if !self.shell.has_pending_persistence() {
            return None;
        }
        let now = Instant::now();
        if !shell_persistence_write_is_allowed(
            self.runtime_output_pressure_active(),
            self.shell_persistence_geometry_is_busy(now),
            self.connect_settle_is_active(now),
        ) {
            return Some(SHELL_PERSISTENCE_RETRY);
        }

        self.flush_pending_session_persistence(cx);
        if self.shell.take_ui_layout_persist_pending() {
            self.queue_settings_save(crate::features::settings::SettingsSaveKind::UiLayout, cx);
        }

        // A session write already in flight leaves its flags set; come back for them
        // rather than waiting for the next unrelated mark.
        self.shell
            .has_pending_persistence()
            .then_some(SHELL_PERSISTENCE_RETRY)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use gpui::{AppContext as _, TestAppContext};
    use nyaterm_core::{AppRuntime, RuntimeMode, uuid};

    use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
    use crate::features::NyaTermApp;
    use crate::features::settings::SettingsPersistenceDomain;

    use super::{SHELL_PERSISTENCE_DEBOUNCE, shell_persistence_write_is_allowed};

    fn unique_test_dir() -> PathBuf {
        // A uuid rather than a clock reading: these tests run in parallel and
        // Windows' ~15ms clock granularity lets a nanosecond timestamp repeat,
        // which would share one config dir and so one settings database.
        std::env::temp_dir().join(format!(
            "nyaterm-persist-debounce-{}-{}",
            std::process::id(),
            uuid()
        ))
    }

    fn app(cx: &mut TestAppContext) -> gpui::Entity<NyaTermApp> {
        let root = unique_test_dir();
        let runtime = AppRuntime::from_parts_for_test(
            RuntimeMode::Portable,
            root.clone(),
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

    /// The replacement for the quiet-gate term this flag used to need.
    ///
    /// `4644195a` fixed the third instance of that defect by adding
    /// `ui_layout_persist_pending` to `runtime_quiet_tick_allowed`, so the idle plane
    /// would run and flush it. There is no runtime tick in this fixture at all, so a
    /// write here can only have come from the debounce task -- which is what makes
    /// the whole defect class structurally impossible rather than patched again.
    #[test]
    fn a_pending_ui_layout_save_is_written_without_any_runtime_tick() {
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx);
        let store = app.update(&mut cx, |app, _| app.store_blocking_client());
        let sync_generation = store.sync_mutation_generation();
        cx.update_entity(&app, |app, cx| {
            app.start_shell_persistence_debounce(cx);
            app.shell.mark_ui_layout_persist_pending();
        });

        // Inside the coalescing window nothing is written yet.
        cx.run_until_parked();
        cx.update_entity(&app, |app, _| {
            assert!(
                app.shell.ui_layout_persist_is_pending(),
                "the debounce window should still be collecting marks"
            );
        });

        cx.executor().advance_clock(SHELL_PERSISTENCE_DEBOUNCE);
        cx.run_until_parked();
        cx.update_entity(&app, |app, _| {
            assert!(
                !app.shell.ui_layout_persist_is_pending(),
                "the debounce task must write a queued UI-layout save on its own"
            );
        });
        store
            .request(0, nyaterm_store::FlushBarrier)
            .unwrap()
            .outcome
            .unwrap();
        assert_eq!(store.sync_mutation_generation(), sync_generation);
    }

    /// A mark arriving while the task is already parked must still get written.
    ///
    /// This is the case the other tests here do not reach: they mark before the task
    /// has run its first iteration, so it finds the flag already set and never has to
    /// be woken at all. In the application the interesting mark is the one an hour
    /// later, when the user drags a panel divider on an idle app -- and that one only
    /// lands if `mark_*` signals the wake. Removing that signal leaves every other
    /// test in this module passing and this one failing.
    #[test]
    fn a_mark_arriving_while_the_task_is_parked_still_gets_written() {
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx);
        cx.update_entity(&app, |app, cx| {
            app.start_shell_persistence_debounce(cx);
        });

        // Let the task reach its park with nothing outstanding.
        cx.run_until_parked();
        cx.update_entity(&app, |app, _| {
            assert!(!app.shell.has_pending_persistence());
        });

        cx.update_entity(&app, |app, _| {
            app.shell.mark_ui_layout_persist_pending();
        });
        cx.executor().advance_clock(SHELL_PERSISTENCE_DEBOUNCE);
        cx.run_until_parked();

        cx.update_entity(&app, |app, _| {
            assert!(
                !app.shell.ui_layout_persist_is_pending(),
                "a mark on an idle app must wake the debounce task, not wait for one"
            );
        });
    }

    /// One write for a burst, which is the point of a debounce.
    #[test]
    fn a_burst_of_marks_collapses_into_one_debounce_window() {
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx);
        cx.update_entity(&app, |app, cx| {
            app.start_shell_persistence_debounce(cx);
            for _ in 0..16 {
                app.shell.mark_session_persistence_dirty();
                app.shell.mark_ui_layout_persist_pending();
            }
        });

        cx.executor().advance_clock(SHELL_PERSISTENCE_DEBOUNCE);
        cx.run_until_parked();

        cx.update_entity(&app, |app, _| {
            assert!(
                !app.shell.has_pending_persistence(),
                "one window should have absorbed the whole burst"
            );
        });
    }

    /// These writes open the config database, so they wait for calm.
    ///
    /// Only the hold is asserted behaviourally: `advance_clock` moves gpui's timer
    /// clock, while the settle deadline is real-time, so a test cannot make it
    /// expire. That the held-off write does eventually happen is covered by
    /// `held_off_writes_are_allowed_once_everything_is_calm` plus the retry the task
    /// schedules whenever this returns `Some`.
    #[test]
    fn persistence_is_held_off_during_connect_settle() {
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx);
        cx.update_entity(&app, |app, cx| {
            app.start_shell_persistence_debounce(cx);
            app.enter_connect_settle();
            app.shell.mark_ui_layout_persist_pending();
        });

        cx.executor().advance_clock(SHELL_PERSISTENCE_DEBOUNCE);
        cx.run_until_parked();

        cx.update_entity(&app, |app, _| {
            assert!(
                app.shell.ui_layout_persist_is_pending(),
                "a write must not open the config DB during connect settle"
            );
        });
    }

    /// The other direction of the same gate, and the matrix the idle plane encoded.
    #[test]
    fn held_off_writes_are_allowed_once_everything_is_calm() {
        assert!(shell_persistence_write_is_allowed(false, false, false));
        assert!(
            !shell_persistence_write_is_allowed(true, false, false),
            "output pressure must hold the write"
        );
        assert!(
            !shell_persistence_write_is_allowed(false, true, false),
            "a window drag or resize must hold the write"
        );
        assert!(
            !shell_persistence_write_is_allowed(false, false, true),
            "connect settle must hold the write"
        );
    }

    /// Debouncing opens a window in which quitting could lose the write.
    ///
    /// The session half is safe because `submit_shutdown_persistence` re-serializes
    /// tabs and layout unconditionally. The UI-layout half is not: it is written only
    /// if its settings domain is dirty, and only `queue_persistence` sets that. So
    /// shutdown has to fold in a mark still sitting inside its debounce window.
    #[test]
    fn shutdown_folds_in_a_ui_layout_save_still_inside_its_debounce_window() {
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx);
        cx.update_entity(&app, |app, cx| {
            app.start_shell_persistence_debounce(cx);
            app.shell.mark_ui_layout_persist_pending();
            assert!(
                !app.settings
                    .dirty_persistence_domains()
                    .contains(&SettingsPersistenceDomain::UiLayout),
                "nothing should have queued the domain yet"
            );

            // Quit without ever advancing the clock.
            let _ = app.submit_shutdown_persistence(true);

            assert!(
                app.settings
                    .dirty_persistence_domains()
                    .contains(&SettingsPersistenceDomain::UiLayout),
                "shutdown must write a UI-layout change the debounce had not reached"
            );
            assert!(!app.shell.ui_layout_persist_is_pending());
        });
    }
}
