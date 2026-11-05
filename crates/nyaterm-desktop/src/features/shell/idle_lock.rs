//! The idle screen-lock clock.
//!
//! `drive_idle_lock` used to run from the runtime tick's idle plane, which meant the
//! lock deadline was checked at whatever cadence the tick happened to be on -- and the
//! idle plane is skipped entirely under output pressure, geometry churn and connect
//! settle, so a terminal producing output could hold the lock off indefinitely. A
//! security deadline should not be a function of how busy the UI is.
//!
//! The clock re-arms lazily rather than being reset by activity. It sleeps to the
//! deadline, and if the user was active in the meantime it simply sleeps again for
//! whatever is left. That keeps all 38 `mark_user_activity` call sites free of any
//! knowledge of the timer -- there is no cancel path to get wrong, and no way for an
//! activity site added later to leave the clock stale.

use std::time::Duration;

use gpui::Context;

use crate::features::NyaTermApp;

/// What the clock should do when it next looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IdleLockDecision {
    /// The idle deadline has passed; lock now.
    Lock,
    /// Activity happened since the timer was armed; come back after this long.
    WaitFor(Duration),
    /// Nothing to watch for: locking is off, or already locked.
    Stop,
}

/// The gate `drive_idle_lock` applied, as a decision the clock can act on.
///
/// `idle_lock_minutes == 0` means the feature is off even when the toggle is on,
/// which is how the settings page expresses "never".
fn idle_lock_decision(
    screen_lock_enabled: bool,
    already_locked: bool,
    idle_lock_minutes: u32,
    idle_for: Duration,
) -> IdleLockDecision {
    if already_locked || !screen_lock_enabled || idle_lock_minutes == 0 {
        return IdleLockDecision::Stop;
    }
    let lock_after = Duration::from_secs(u64::from(idle_lock_minutes) * 60);
    match lock_after.checked_sub(idle_for) {
        Some(remaining) if !remaining.is_zero() => IdleLockDecision::WaitFor(remaining),
        _ => IdleLockDecision::Lock,
    }
}

impl NyaTermApp {
    /// Watch the idle deadline while screen locking is on.
    ///
    /// Idempotent, so it can be called from anywhere a lock input changed. Stops
    /// itself when locking is off or the screen is already locked, so an app with the
    /// feature disabled costs nothing.
    pub(in crate::features) fn ensure_idle_lock_clock(&mut self, cx: &mut Context<Self>) {
        if self.shell.idle_lock_clock_is_armed() {
            return;
        }
        let IdleLockDecision::WaitFor(mut delay) = self.idle_lock_decision() else {
            return;
        };
        self.shell.set_idle_lock_clock_armed(true);
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(delay).await;
                // `update_in` rather than `update`: locking focuses the password
                // field, which needs the window.
                let Ok(next) = NyaTermApp::update_in_main_window(&this, cx, |this, window, cx| {
                    this.tick_idle_lock(window, cx)
                }) else {
                    break;
                };
                match next {
                    Some(remaining) => delay = remaining,
                    None => break,
                }
            }
        })
        .detach();
    }

    fn idle_lock_decision(&self) -> IdleLockDecision {
        idle_lock_decision(
            self.settings.summary().enable_idle_lock,
            self.security.screen_locked(),
            self.settings.summary().idle_lock_minutes,
            self.security.screen_lock_idle_for(),
        )
    }

    /// One look at the deadline. Returns how long to wait again, or `None` to stop.
    fn tick_idle_lock(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) -> Option<Duration> {
        match self.idle_lock_decision() {
            IdleLockDecision::WaitFor(remaining) => Some(remaining),
            IdleLockDecision::Stop => {
                self.shell.set_idle_lock_clock_armed(false);
                None
            }
            IdleLockDecision::Lock => {
                self.shell.set_idle_lock_clock_armed(false);
                if self.lock_screen_for_idle(window, cx) {
                    cx.notify();
                }
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{IdleLockDecision, idle_lock_decision};

    const FIVE_MINUTES: Duration = Duration::from_secs(5 * 60);

    #[test]
    fn the_clock_waits_out_the_remaining_idle_time() {
        assert_eq!(
            idle_lock_decision(true, false, 5, Duration::ZERO),
            IdleLockDecision::WaitFor(FIVE_MINUTES)
        );
        // Activity happened after the timer was armed, so this look defers rather
        // than locking. That lazy re-arm is why no activity site has to know about
        // the clock.
        assert_eq!(
            idle_lock_decision(true, false, 5, Duration::from_secs(60)),
            IdleLockDecision::WaitFor(Duration::from_secs(4 * 60))
        );
    }

    #[test]
    fn the_clock_locks_once_the_deadline_is_reached() {
        assert_eq!(
            idle_lock_decision(true, false, 5, FIVE_MINUTES),
            IdleLockDecision::Lock
        );
        assert_eq!(
            idle_lock_decision(true, false, 5, FIVE_MINUTES * 3),
            IdleLockDecision::Lock,
            "an overshoot -- a suspended machine, say -- must lock, not wrap around"
        );
    }

    #[test]
    fn the_clock_stops_when_there_is_nothing_to_watch() {
        assert_eq!(
            idle_lock_decision(false, false, 5, FIVE_MINUTES),
            IdleLockDecision::Stop,
            "locking is switched off"
        );
        assert_eq!(
            idle_lock_decision(true, false, 0, FIVE_MINUTES),
            IdleLockDecision::Stop,
            "zero minutes is how the settings page says never"
        );
        assert_eq!(
            idle_lock_decision(true, true, 5, FIVE_MINUTES),
            IdleLockDecision::Stop,
            "already locked; unlocking restarts the clock"
        );
    }
}
