//! The network tools configuration windows.
//!
//! Each service owns one child window, opened from the panel row for that
//! service. The lifecycle mirrors the asset batch editor's contract: a per
//! service slot, a deferred open, and a single teardown entry point per service
//! that restores or discards its draft. Locking closes every open window and
//! remembers the set to reopen after unlocking.

use std::rc::Rc;

use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, IntoElement, Render, Subscription, Window,
    div, prelude::*, px, rgb,
};
use nyaterm_transport::networktools::NetworkToolKind;
use nyaterm_ui::notification::NyaNotificationKind;
use nyaterm_ui::{NyaWindowHandle, activate_child_window, nya_root};
use rust_i18n::t;

use crate::features::NyaTermApp;
use crate::features::networktools::common::ids;
use crate::features::view_widgets::{
    ChildWindowChrome, ChildWindowCloseHandler, ChildWindowSpec, child_window_header,
    child_window_options, child_window_root, focus_child_window_shell_if_idle,
};

pub(in crate::features) struct NetworkToolsConfigWindow {
    kind: NetworkToolKind,
    app: Entity<NyaTermApp>,
    shell_focus: FocusHandle,
    chrome: ChildWindowChrome,
    _app_subscription: Subscription,
}

impl NetworkToolsConfigWindow {
    fn new(
        kind: NetworkToolKind,
        app: Entity<NyaTermApp>,
        chrome: ChildWindowChrome,
        cx: &mut Context<Self>,
    ) -> Self {
        let app_subscription = cx.observe(&app, |_, _, cx| cx.notify());
        Self {
            kind,
            app,
            shell_focus: cx.focus_handle(),
            chrome,
            _app_subscription: app_subscription,
        }
    }
}

impl Render for NetworkToolsConfigWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.app.read(cx).networktools.is_config_open(self.kind) {
            window.defer(cx, |window, _| window.remove_window());
            return div().size_full().into_any_element();
        }

        let kind = self.kind;
        let (palette, font, font_size, title) = self.app.read_with(cx, |app, _| {
            (
                app.theme_palette(),
                app.gpui_ui_font().font(),
                app.settings.summary().ui_font_size.clamp(12, 24) as f32,
                t!(kind.name_key()).to_string(),
            )
        });
        window.set_window_title(&title);
        let content = self
            .app
            .update(cx, |app, cx| app.networktools_config_window_view(kind, cx));
        let close_app = self.app.clone();
        let on_close: ChildWindowCloseHandler =
            Rc::new(move |window: &mut Window, cx: &mut App| {
                close_app.update(cx, |app, cx| {
                    app.end_networktools_config_session(kind, true, cx);
                });
                window.remove_window();
            });
        let header_close = on_close.clone();
        focus_child_window_shell_if_idle(&self.shell_focus, window, cx);

        child_window_root(&self.shell_focus, true, on_close)
            .bg(rgb(palette.bg))
            .text_color(rgb(palette.text))
            .font(font)
            .text_size(px(font_size))
            .child(child_window_header(
                palette,
                title,
                None,
                self.chrome,
                window,
                cx,
                move |_, window, cx| header_close(window, cx),
            ))
            .child(div().flex_1().min_h_0().overflow_hidden().child(content))
            .into_any_element()
    }
}

impl NyaTermApp {
    pub(in crate::features) fn activate_networktools_config_window(
        &mut self,
        kind: NetworkToolKind,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(handle) = self.networktools.config_window_handle(kind) else {
            return false;
        };
        activate_child_window(
            &cx.entity(),
            handle,
            move |app: &mut NyaTermApp| app.networktools.config_window_slot(kind),
            cx,
        );
        true
    }

    /// Open the service's configuration window, or raise it when already
    /// present. Other services' windows are independent and may stay open.
    pub(in crate::features) fn open_networktools_config(
        &mut self,
        kind: NetworkToolKind,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.activate_networktools_config_window(kind, cx) {
            return;
        }
        if self.networktools.config_window_is_pending(kind) {
            return;
        }
        if !self.networktools.begin_window_open(kind) {
            return;
        }
        self.networktools.begin_config_edit(kind);
        self.forget_text_inputs(&ids::input_prefix(kind));
        self.refresh_networktools_log(kind, cx);
        cx.notify();
        let app = cx.entity();
        cx.defer(move |cx| open_networktools_config_now(app, kind, cx));
    }

    /// The single teardown path for one service: restore or discard its draft,
    /// drop its input registry entries and make its window remove itself.
    pub(in crate::features) fn end_networktools_config_session(
        &mut self,
        kind: NetworkToolKind,
        restore: bool,
        cx: &mut Context<Self>,
    ) {
        if restore {
            self.networktools.discard_config_edits(kind);
        }
        self.forget_text_inputs(&ids::input_prefix(kind));
        let handle = self.networktools.end_config_session(kind);
        if let Some(handle) = handle {
            cx.defer(move |cx| {
                let _ = handle.update(cx, |_, window, _| window.remove_window());
            });
        }
        cx.notify();
    }

    /// Close every configuration window for the lock screen but keep the
    /// drafts, the input registries and the intent to reopen them.
    pub(in crate::features) fn networktools_suspend_for_lock(&mut self, cx: &mut Context<Self>) {
        let handles = self.networktools.suspend_config_windows();
        if handles.is_empty() {
            return;
        }
        cx.defer(move |cx| {
            for handle in handles {
                let _ = handle.update(cx, |_, window, _| window.remove_window());
            }
        });
        cx.notify();
    }

    pub(in crate::features) fn networktools_resume_after_unlock(&mut self, cx: &mut Context<Self>) {
        let mut reopened = false;
        for kind in self.networktools.take_reopen_kinds() {
            if !self.networktools.is_config_open(kind) {
                continue;
            }
            if !self.networktools.begin_window_open(kind) {
                continue;
            }
            reopened = true;
            let app = cx.entity();
            cx.defer(move |cx| open_networktools_config_now(app, kind, cx));
        }
        if reopened {
            cx.notify();
        }
    }
}

fn open_networktools_config_now(app: Entity<NyaTermApp>, kind: NetworkToolKind, cx: &mut App) {
    let (pending, open) = {
        let state = &app.read(cx).networktools;
        (
            state.config_window_is_pending(kind),
            state.is_config_open(kind),
        )
    };
    if !pending || !open {
        return;
    }

    let title = t!(kind.name_key()).to_string();
    let spec = ChildWindowSpec::modal_editor(title, 900., 720.).min_size(760., 520.);
    let chrome = spec.chrome();
    let parent = app.read(cx).shell.main_window();
    let options = child_window_options(&spec, parent, cx);
    let close_app = app.clone();
    let view_app = app.clone();
    let result: anyhow::Result<NyaWindowHandle> = cx.open_window(options, move |window, cx| {
        window.on_window_should_close(cx, move |_, cx| {
            close_app.update(cx, |app, cx| {
                app.end_networktools_config_session(kind, true, cx);
            });
            true
        });
        let view = cx.new(|cx| NetworkToolsConfigWindow::new(kind, view_app, chrome, cx));
        cx.new(|cx| nya_root(view, window, cx))
    });

    app.update(cx, |app, cx| match result {
        Ok(handle) => {
            app.networktools.attach_config_window(kind, handle);
            cx.notify();
        }
        Err(error) => {
            app.networktools.fail_config_window(kind);
            let message = format!("{}: {error}", t!("networktools.common.status.saveFailed"));
            app.networktools
                .set_status_message(kind, Some(message.clone()));
            app.notify_background_operation(
                ids::status_notification(kind),
                NyaNotificationKind::Error,
                message,
                cx,
            );
            cx.notify();
        }
    });
}
