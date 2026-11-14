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
use crate::models::ConnectionEditorField;

pub(in crate::features::connections) struct ConnectionEditorWindow {
    app: Entity<NyaTermApp>,
    shell_focus: FocusHandle,
    chrome: ChildWindowChrome,
    _app_subscription: Subscription,
    /// Focus is per-window, so the field the main window focused means nothing
    /// here; this window has to claim it on its own first frame.
    focused_initial_field: bool,
}

impl ConnectionEditorWindow {
    fn new(app: Entity<NyaTermApp>, chrome: ChildWindowChrome, cx: &mut Context<Self>) -> Self {
        let app_subscription = cx.observe(&app, |_, _, cx| cx.notify());
        Self {
            app,
            shell_focus: cx.focus_handle(),
            chrome,
            _app_subscription: app_subscription,
            focused_initial_field: false,
        }
    }
}

impl Render for ConnectionEditorWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(editor) = self.app.read(cx).connection_state.active_editor_draft() else {
            self.app.update(cx, |app, cx| {
                app.connection_state.clear_editor_window();
                cx.notify();
            });
            window.defer(cx, |window, _| window.remove_window());
            return div().size_full().into_any_element();
        };

        let (palette, font, font_size, title) = self.app.read_with(cx, |app, _| {
            (
                app.theme_palette(),
                app.gpui_ui_font().font(),
                app.settings.summary().ui_font_size.clamp(12, 24) as f32,
                app.connection_editor_title().to_string(),
            )
        });
        window.set_window_title(&title);
        if !self.focused_initial_field {
            self.focused_initial_field = true;
            // Defaults mode has no basic tab, so there is no name to preselect.
            let defaults_mode = self
                .app
                .read(cx)
                .connection_state
                .active_editor_draft()
                .is_some_and(|editor| editor.defaults_mode);
            let field = (!defaults_mode)
                .then(|| {
                    self.app
                        .read(cx)
                        .connection_state
                        .editor_fields()
                        .get(&ConnectionEditorField::Name)
                        .cloned()
                })
                .flatten();
            if let Some(field) = field {
                window.focus(&field.read(cx).focus_handle(), cx);
                field.update(cx, |field, cx| field.select_all(window, cx));
            }
        }
        let content = self.app.update(cx, |app, cx| {
            app.connection_editor_window_view(
                editor,
                (
                    f32::from(window.viewport_size().width),
                    f32::from(window.viewport_size().height),
                ),
                cx,
            )
        });
        let close_app = self.app.clone();
        let on_close: ChildWindowCloseHandler =
            Rc::new(move |window: &mut Window, cx: &mut App| {
                close_app.update(cx, |app, cx| app.close_connection_editor(cx));
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
    pub(in crate::features) fn connection_editor_title(&self) -> Cow<'static, str> {
        if self
            .connection_state
            .active_editor_draft()
            .is_some_and(|editor| editor.defaults_mode)
        {
            t!("menu.defaultSessionSettings")
        } else if self.connection_state.editor_is_editing_saved_connection() {
            t!("dialog.editConnection")
        } else {
            t!("dialog.newConnection")
        }
    }

    /// Raise the editor window if one is already open.
    ///
    /// This is what stops a second "new connection" from starting a competing
    /// draft: there is one draft slot, so the second request has to land on the
    /// window that already owns it.
    pub(in crate::features) fn activate_connection_editor_window(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(handle) = self.connection_state.editor_window_handle() else {
            return false;
        };
        activate_child_window(
            &cx.entity(),
            handle,
            |app: &mut NyaTermApp| Some(app.connection_state.editor_window_slot()),
            cx,
        );
        true
    }

    pub(in crate::features) fn open_connection_editor_window(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.activate_connection_editor_window(cx) {
            return true;
        }
        if !self.connection_state.begin_editor_window_open() {
            return true;
        }
        cx.notify();
        let app = cx.entity();
        cx.defer(move |cx| {
            let should_open = app.read(cx).connection_state.editor_window_open_pending();
            if should_open {
                open_connection_editor_window_now_from_app(app, cx);
            }
        });
        true
    }
}

fn open_connection_editor_window_now_from_app(app: Entity<NyaTermApp>, cx: &mut App) {
    if app.read(cx).connection_state.editor_has_window() {
        app.update(cx, |app, cx| {
            app.connection_state.clear_editor_window_pending();
            app.activate_connection_editor_window(cx);
            cx.notify();
        });
        return;
    }
    if !app.read(cx).connection_state.editor_has_draft() {
        app.update(cx, |app, cx| {
            app.connection_state.clear_editor_window_pending();
            cx.notify();
        });
        return;
    }

    let title = app.read(cx).connection_editor_title().to_string();
    let spec = ChildWindowSpec::modal_editor(title, 900., 680.).min_size(420., 480.);
    let chrome = spec.chrome();
    let parent = app.read(cx).shell.main_window();
    let options = child_window_options(&spec, parent, cx);
    let close_app = app.clone();
    let view_app = app.clone();
    let result: anyhow::Result<NyaWindowHandle> = cx.open_window(options, move |window, cx| {
        window.on_window_should_close(cx, move |_, cx| {
            close_app.update(cx, |app, cx| {
                app.connection_state.close_editor();
                app.shell
                    .set_status(t!("dialog.connectionEditorClosed").to_string());
                cx.notify();
            });
            true
        });
        let editor_focus = view_app.read(cx).connection_state.editor_focus_handle();
        window.focus(&editor_focus, cx);
        let view = cx.new(|cx| ConnectionEditorWindow::new(view_app, chrome, cx));
        cx.new(|cx| nya_root(view, window, cx))
    });

    app.update(cx, |app, cx| match result {
        Ok(handle) => {
            app.connection_state.attach_editor_window(handle);
            cx.notify();
        }
        Err(error) => {
            app.connection_state.clear_editor_window();
            app.shell
                .set_status(t!("dialog.connectionEditorOpenFailed", error = error));
            cx.notify();
        }
    });
}
