//! Deferred focus for elements that do not exist yet.
//!
//! Three features ask for focus from code that cannot take it: an AI chat box, a
//! transfer rename field, and an SSH credential prompt. Each sets a pending flag and
//! something else applies it later, because `focus_text_input_if_present` can only
//! succeed once the input has been built -- which happens as a result of a paint.
//!
//! That "something else" was the runtime tick's idle plane, which is why all three
//! flags had to be named in `runtime_quiet_tick_allowed`: a request made on an
//! otherwise-quiet app would otherwise wait up to the 500ms quiet interval to be
//! honoured, and a focus that arrives half a second after the click is a focus the
//! user has already worked around. All three terms are gone with this clock.

use std::time::Duration;

use gpui::{Context, Window};

use crate::features::NyaTermApp;

/// How long to wait before trying a pending focus again.
///
/// A request becomes satisfiable when the element exists, so the useful granularity is
/// a frame rather than anything finer. The clock only runs while a request is
/// outstanding, which is a span of a frame or two in practice.
const PENDING_FOCUS_RETRY_INTERVAL: Duration = Duration::from_millis(16);

impl NyaTermApp {
    /// Apply pending focus requests until none are left.
    ///
    /// Idempotent, so it can be called from anywhere a request might have been made
    /// or become satisfiable -- including `render`, which is where "the element now
    /// exists" first becomes true.
    pub(in crate::features) fn ensure_pending_focus_clock(&mut self, cx: &mut Context<Self>) {
        if self.security.screen_locked()
            || self.shell.pending_focus_clock_is_armed()
            || !self.has_pending_focus_request()
        {
            return;
        }
        self.shell.set_pending_focus_clock_armed(true);
        cx.spawn(async move |this, cx| {
            loop {
                // `update_in`: taking focus needs the window.
                let Ok(still_pending) =
                    NyaTermApp::update_in_main_window(&this, cx, |this, window, cx| {
                        if this.security.screen_locked() {
                            this.shell.set_pending_focus_clock_armed(false);
                            return false;
                        }
                        if this.drive_pending_focus(window, cx) {
                            cx.notify();
                        }
                        let pending = this.has_pending_focus_request();
                        if !pending {
                            this.shell.set_pending_focus_clock_armed(false);
                        }
                        pending
                    })
                else {
                    break;
                };
                if !still_pending {
                    break;
                }
                cx.background_executor()
                    .timer(PENDING_FOCUS_RETRY_INTERVAL)
                    .await;
            }
        })
        .detach();
    }

    pub(in crate::features) fn has_pending_focus_request(&self) -> bool {
        self.ai.chat_focus_is_pending()
            || self.transfer.rename_focus_is_pending()
            || self.session.prompt_credential_focus_is_pending()
    }

    /// Take whichever pending focus can be taken now. Returns whether anything moved.
    ///
    /// A request that cannot be satisfied yet stays pending on purpose: the rename
    /// field may not be built, and the credential prompt may not be the active one.
    pub(in crate::features) fn drive_pending_focus(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.security.screen_locked() {
            return false;
        }
        if !self.has_pending_focus_request() {
            return false;
        }
        let mut dirty = false;
        if self.ai.take_chat_focus_request() {
            window.focus(self.ai.chat_focus(), cx);
            dirty = true;
        }
        if let Some(input_id) = self.transfer.pending_rename_input_id()
            && let Some(field) = self.existing_text_input(&input_id)
        {
            field.update(cx, |field, cx| {
                field.select_all_with_cursor_at_end(window, cx)
            });
            self.transfer.finish_rename_focus();
            dirty = true;
        }
        if self.session.prompt_credential_focus_is_pending()
            && (self.session.prompt_has_active_credential()
                || self.session.prompt_has_active_keyboard_interactive())
        {
            self.focus_active_ssh_prompt_input(window, cx);
            self.session.prompt_finish_credential_focus();
            dirty = true;
        }
        dirty
    }
}

#[cfg(test)]
mod tests {
    use gpui::{AppContext as _, TestAppContext};
    use nyaterm_core::{AppRuntime, RuntimeMode, uuid};

    use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
    use crate::features::NyaTermApp;

    fn app(cx: &mut TestAppContext) -> gpui::Entity<NyaTermApp> {
        // A uuid rather than a clock reading: these tests run in parallel and
        // Windows' ~15ms clock granularity lets a nanosecond timestamp repeat.
        let root = std::env::temp_dir().join(format!(
            "nyaterm-pending-focus-{}-{}",
            std::process::id(),
            uuid()
        ));
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

    /// No outstanding request means no clock, which is the state the app is in almost
    /// all of the time.
    #[test]
    fn no_focus_clock_runs_without_a_request() {
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx);
        cx.update_entity(&app, |app, cx| {
            assert!(!app.has_pending_focus_request());
            app.ensure_pending_focus_clock(cx);
            assert!(!app.shell.pending_focus_clock_is_armed());
        });
    }

    /// A request that cannot be honoured yet keeps the clock, rather than being
    /// dropped or spun on.
    #[test]
    fn an_unsatisfiable_request_keeps_the_clock_armed() {
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx);
        cx.update_entity(&app, |app, cx| {
            // A credential-prompt focus with no active prompt cannot be taken: the
            // idle plane left it pending for the same reason.
            app.session.prompt_request_credential_focus_for_test();
            assert!(app.has_pending_focus_request());
            app.ensure_pending_focus_clock(cx);
            assert!(
                app.shell.pending_focus_clock_is_armed(),
                "an outstanding request must be retried, not dropped"
            );
        });
    }
}
