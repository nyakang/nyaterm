use rust_i18n::t;

use std::borrow::Cow;

use std::rc::Rc;

use gpui::{
    App, AppContext, Context, Entity, FocusHandle, IntoElement, Render, Subscription, Window, div,
    prelude::*, px, rgb,
};
use nyaterm_ui::{NyaWindowHandle, activate_child_window, nya_root};

use crate::features::{
    NyaTermApp,
    view_widgets::{
        ChildWindowChrome, ChildWindowCloseHandler, ChildWindowSpec, child_window_header,
        child_window_options, child_window_root, focus_child_window_shell_if_idle,
    },
};

pub(in crate::features::commands) struct QuickCommandWindow {
    app: Entity<NyaTermApp>,
    shell_focus: FocusHandle,
    chrome: ChildWindowChrome,
    _app_subscription: Subscription,
}

impl QuickCommandWindow {
    fn new(app: Entity<NyaTermApp>, chrome: ChildWindowChrome, cx: &mut Context<Self>) -> Self {
        let app_subscription = cx.observe(&app, |_, _, cx| cx.notify());
        Self {
            app,
            shell_focus: cx.focus_handle(),
            chrome,
            _app_subscription: app_subscription,
        }
    }
}

impl Render for QuickCommandWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.app.read(cx).commands.quick_editor().is_none() {
            self.app.update(cx, |app, cx| {
                app.commands.close_quick_editor();
                cx.notify();
            });
            window.defer(cx, |window, _| window.remove_window());
            return div().size_full().into_any_element();
        }

        let viewport_width = f32::from(window.viewport_size().width);
        let (palette, font, font_size, title) = self.app.read_with(cx, |app, _| {
            (
                app.theme_palette(),
                app.gpui_ui_font().font(),
                app.settings.summary().ui_font_size.clamp(12, 24) as f32,
                app.quick_command_editor_title().to_string(),
            )
        });
        window.set_window_title(&title);
        let content = self.app.update(cx, |app, cx| {
            app.quick_command_editor_window_view(viewport_width, cx)
        });
        let close_app = self.app.clone();
        let on_close: ChildWindowCloseHandler =
            Rc::new(move |window: &mut Window, cx: &mut App| {
                close_app.update(cx, |app, cx| app.close_quick_command_editor(cx));
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
                move |_, window, cx| header_close(window, cx),
            ))
            .child(div().flex_1().min_h_0().overflow_hidden().child(content))
            .into_any_element()
    }
}

impl NyaTermApp {
    pub(in crate::features) fn quick_command_editor_title(&self) -> Cow<'static, str> {
        if self
            .commands
            .quick_editor()
            .is_some_and(|editor| editor.original.is_some())
        {
            t!("quickCommands.editCommand")
        } else {
            t!("quickCommands.addCommand")
        }
    }

    /// Raise the editor window if one is already open.
    ///
    /// There is a single quick-command draft, so a second open request has to
    /// land on the window that already owns it rather than start a rival draft.
    pub(in crate::features) fn activate_quick_command_window(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(handle) = self.commands.quick_editor_window() else {
            return false;
        };
        activate_child_window(
            &cx.entity(),
            handle,
            |app: &mut NyaTermApp| Some(app.commands.quick_editor_window_slot()),
            cx,
        );
        true
    }

    pub(in crate::features) fn open_quick_command_window(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.activate_quick_command_window(cx) {
            return true;
        }
        if self.commands.quick_editor_window_is_pending() {
            return true;
        }

        if !self.commands.request_quick_editor_window() {
            return false;
        }
        cx.notify();
        let app = cx.entity();
        cx.defer(move |cx| {
            let should_open = app.read(cx).commands.quick_editor_window_is_pending();
            if should_open {
                open_quick_command_window_now_from_app(app, cx);
            }
        });
        true
    }
}

fn open_quick_command_window_now_from_app(app: Entity<NyaTermApp>, cx: &mut App) {
    if app.read(cx).commands.quick_editor_window().is_some() {
        app.update(cx, |app, cx| {
            app.commands.cancel_quick_editor_window_request();
            app.activate_quick_command_window(cx);
            cx.notify();
        });
        return;
    }
    if app.read(cx).commands.quick_editor().is_none() {
        app.update(cx, |app, cx| {
            app.commands.cancel_quick_editor_window_request();
            cx.notify();
        });
        return;
    }

    let title = app.read(cx).quick_command_editor_title().to_string();
    let spec = ChildWindowSpec::modal_editor(title, 540., 720.).min_size(420., 560.);
    let chrome = spec.chrome();
    let parent = app.read(cx).shell.main_window();
    let options = child_window_options(&spec, parent, cx);
    let close_app = app.clone();
    let view_app = app.clone();
    let result: anyhow::Result<NyaWindowHandle> = cx.open_window(options, move |window, cx| {
        window.on_window_should_close(cx, move |_, cx| {
            close_app.update(cx, |app, cx| {
                app.commands.close_quick_editor();
                app.shell
                    .set_status("quick command editor closed".to_string());
                cx.notify();
            });
            true
        });
        let editor_focus = view_app.read(cx).commands.quick_editor_focus().clone();
        window.focus(&editor_focus, cx);
        let view = cx.new(|cx| QuickCommandWindow::new(view_app, chrome, cx));
        cx.new(|cx| nya_root(view, window, cx))
    });

    app.update(cx, |app, cx| match result {
        Ok(handle) => {
            app.commands.finish_quick_editor_window_open(Some(handle));
            cx.notify();
        }
        Err(error) => {
            app.commands.finish_quick_editor_window_open(None);
            app.shell
                .set_status(format!("failed to open quick command window: {error}"));
            cx.notify();
        }
    });
}
