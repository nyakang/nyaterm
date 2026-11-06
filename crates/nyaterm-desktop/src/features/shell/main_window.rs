//! Async access to `NyaTermApp` inside the window it renders in.

use anyhow::Context as _;
use gpui::{AppContext, Context, WeakEntity, Window};

use crate::features::NyaTermApp;

impl NyaTermApp {
    /// `WeakEntity::update_in`, pinned to the main window.
    ///
    /// GPUI resolves `update_in` to whichever window last drew while reading the
    /// entity. Child windows render app state, so after one has drawn, a plain
    /// `update_in` hands main-window work -- native RDP hosting, focus, the lock
    /// screen -- to the child window instead.
    pub(in crate::features) fn update_in_main_window<C: AppContext, R>(
        this: &WeakEntity<Self>,
        cx: &mut C,
        update: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) -> R,
    ) -> anyhow::Result<R> {
        let Some(main) = this.read_with(cx, |this, _| this.shell.main_window())? else {
            return this.update_in(cx, update);
        };
        let entity = this.upgrade().context("entity released")?;
        cx.update_window(main.into(), |_, window, cx| {
            entity.update(cx, |this, cx| update(this, window, cx))
        })
    }
}

#[cfg(test)]
mod tests {
    use gpui::{AnyWindowHandle, AppContext as _, Entity, TestAppContext, VisualTestContext};
    use nyaterm_core::test_support::TestTempDir;

    use crate::features::NyaTermApp;
    use crate::features::test_support::app_with_visible_local_session;

    fn open_window_reading(app: &Entity<NyaTermApp>, cx: &mut TestAppContext) -> AnyWindowHandle {
        let app = app.clone();
        let handle = cx.add_window(move |window, cx| {
            let host = cx.new(|_| ReadsApp(app));
            nyaterm_ui::nya_root(host, window, cx)
        });
        VisualTestContext::from_window(handle.into(), cx).update(|window, cx| {
            let _ = window.draw(cx);
        });
        handle.into()
    }

    /// A child window that renders app state becomes the entity's "current"
    /// window in GPUI. Async main-window work must still reach the main window.
    #[test]
    fn main_window_updates_survive_a_child_window_rendering_app_state() {
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let root = TestTempDir::new("nyaterm-main-window-update");
        let app = app_with_visible_local_session(&mut cx, root.path(), "local");

        let main = open_window_reading(&app, &mut cx);
        app.update(&mut cx, |app, _| {
            app.shell.remember_main_window(main.downcast().unwrap())
        });
        let child = open_window_reading(&app, &mut cx);

        let weak = app.downgrade();
        let plain = weak
            .update_in(&mut cx, |_, window, _| window.window_handle())
            .unwrap();
        assert_eq!(plain, child, "GPUI rebinds the entity to the drawing child");

        let pinned = NyaTermApp::update_in_main_window(&weak, &mut cx, |_, window, _| {
            window.window_handle()
        })
        .unwrap();
        assert_eq!(pinned, main);
    }

    struct ReadsApp(Entity<NyaTermApp>);

    impl gpui::Render for ReadsApp {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            let _ = self.0.read(cx).shell.main_window();
            gpui::div()
        }
    }
}
