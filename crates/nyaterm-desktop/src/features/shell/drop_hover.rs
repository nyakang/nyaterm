//! Clearing a drop-target highlight when the drag goes away.
//!
//! A drag over the terminal or the transfer browser sets a hover highlight, while a
//! session-tab drag records its source so the source tab can be dimmed. Dropping clears
//! that transient state through the drop handler, but a drag can also end *without* a
//! drop on our element -- dragged back out of the window, or cancelled -- and GPUI
//! reports that only as `has_active_drag` becoming false. There is no drag-end event to
//! hang this on, so it stays a poll.
//!
//! What changes is the scope. The runtime tick's visual plane checked
//! `has_active_drag` on every tick regardless, and both hover flags had to be named in
//! the tick's due-work predicate so a stale highlight would be noticed at all. Now the
//! poll exists only while a highlight is actually up -- a span measured in the seconds a
//! user spends dragging -- and nothing wakes for it otherwise.

use std::time::Duration;

use gpui::Context;

use crate::features::NyaTermApp;

/// How often to look for the drag having ended while a highlight is up.
///
/// A frame: the highlight is a visual affordance, so noticing within one paint is both
/// enough and the finest that could matter.
const DROP_HOVER_POLL_INTERVAL: Duration = Duration::from_millis(16);

impl NyaTermApp {
    /// Watch for the drag ending while a drop highlight is showing.
    ///
    /// Idempotent, and armed from the places that set a highlight.
    pub(in crate::features) fn ensure_drop_hover_clock(&mut self, cx: &mut Context<Self>) {
        if self.shell.drop_hover_clock_is_armed() || !self.has_drop_hover() {
            return;
        }
        self.shell.set_drop_hover_clock_armed(true);
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(DROP_HOVER_POLL_INTERVAL)
                    .await;
                let Ok(keep_running) = this.update(cx, |this, cx| this.tick_drop_hover(cx)) else {
                    break;
                };
                if !keep_running {
                    break;
                }
            }
        })
        .detach();
    }

    pub(in crate::features) fn has_drop_hover(&self) -> bool {
        self.terminal.terminal_file_drop_hover_is_pending()
            || self.transfer.browser_external_drop_hover_is_pending()
            || self.session.tab_drag_is_pending()
    }

    /// Returns whether the clock should keep running.
    fn tick_drop_hover(&mut self, cx: &mut Context<Self>) -> bool {
        if cx.has_active_drag() {
            // Still dragging: the highlight is correct, so keep watching.
            let running = self.has_drop_hover();
            if !running {
                self.shell.set_drop_hover_clock_armed(false);
            }
            return running;
        }
        let mut dirty = self.terminal.clear_terminal_file_drop_hover();
        let transfer_dirty = self.transfer.set_browser_external_drop_hover(false);
        dirty |= transfer_dirty;
        dirty |= self.session.clear_tab_drag();
        if dirty {
            if transfer_dirty {
                self.defer_transfer_panel_snapshot_flush(cx);
            }
            cx.notify();
        }
        let running = self.has_drop_hover();
        if !running {
            self.shell.set_drop_hover_clock_armed(false);
        }
        running
    }
}
