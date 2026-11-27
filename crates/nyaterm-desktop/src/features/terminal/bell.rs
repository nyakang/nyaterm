//! Window-wide terminal bell policy and native sound delivery.

use std::time::{Duration, Instant};

use gpui::{AppContext as _, Context};

use crate::features::NyaTermApp;

const BELL_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Default)]
pub(super) struct TerminalBellState {
    last_played: Option<Instant>,
    pending: bool,
}

impl TerminalBellState {
    fn request(&mut self, now: Instant) -> bool {
        if self.pending
            || self
                .last_played
                .is_some_and(|last| now.saturating_duration_since(last) < BELL_INTERVAL)
        {
            return false;
        }
        self.pending = true;
        true
    }

    fn finish(&mut self, now: Instant, allowed: bool) -> bool {
        if !std::mem::take(&mut self.pending) || !allowed {
            return false;
        }
        self.last_played = Some(now);
        true
    }
}

impl NyaTermApp {
    fn terminal_bell_allowed(&self, session_id: &str) -> bool {
        self.settings.summary().terminal_bell_enabled
            && !self.security.screen_locked()
            && self.session.has_session(session_id)
    }

    pub(super) fn request_terminal_bell(&mut self, session_id: &str, cx: &mut Context<Self>) {
        let Some(main) = self.shell.main_window() else {
            return;
        };
        if !self.terminal_bell_allowed(session_id) || !self.terminal.bell.request(Instant::now()) {
            return;
        }
        let this = cx.weak_entity();
        let session_id = session_id.to_owned();
        // Pin delivery to the owning main window, not a child that last rendered
        // this entity. Defer until both the current entity and window are released.
        cx.defer(move |cx| {
            let result = cx.update_window(main.into(), |_, window, cx| {
                let play = this.update(cx, |app, _| {
                    let allowed = app.shell.main_window() == Some(main)
                        && app.terminal_bell_allowed(&session_id);
                    app.terminal.bell.finish(Instant::now(), allowed)
                });
                if matches!(play, Ok(true)) {
                    window.play_system_bell();
                }
            });
            if result.is_err() {
                let _ = this.update(cx, |app, _| {
                    app.terminal.bell.finish(Instant::now(), false);
                });
            }
        });
    }
}

#[cfg(test)]
mod tests;
