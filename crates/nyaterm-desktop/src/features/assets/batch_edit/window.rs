//! The batch editor's own window.
//!
//! Mirrors `ConnectionEditorWindow`: same child-window contract, same header,
//! same focus policy. The form itself lives in `view.rs`.

use std::rc::Rc;

use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, IntoElement, Render, Subscription, Window,
    div, prelude::*, px, rgb,
};
use nyaterm_ui::{NyaWindowHandle, activate_child_window, nya_root};
use rust_i18n::t;

use crate::features::NyaTermApp;
use crate::features::view_widgets::{
    ChildWindowChrome, ChildWindowCloseHandler, ChildWindowSpec, child_window_header,
    child_window_options, child_window_root, focus_child_window_shell_if_idle,
};

pub(in crate::features) struct AssetBatchEditWindow {
    app: Entity<NyaTermApp>,
    shell_focus: FocusHandle,
    chrome: ChildWindowChrome,
    generation: u64,
    _app_subscription: Subscription,
}

impl AssetBatchEditWindow {
    fn new(app: Entity<NyaTermApp>, chrome: ChildWindowChrome, cx: &mut Context<Self>) -> Self {
        let generation = app.read(cx).start_workspace.asset_batch().generation();
        let app_subscription = cx.observe(&app, |_, _, cx| cx.notify());
        Self {
            app,
            shell_focus: cx.focus_handle(),
            chrome,
            generation,
            _app_subscription: app_subscription,
        }
    }
}

impl Render for AssetBatchEditWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self
            .app
            .read(cx)
            .start_workspace
            .asset_batch()
            .matches_generation(self.generation)
        {
            window.defer(cx, |window, _| window.remove_window());
            return div().size_full().into_any_element();
        }

        let (palette, font, font_size, title) = self.app.read_with(cx, |app, _| {
            (
                app.theme_palette(),
                app.gpui_ui_font().font(),
                app.settings.summary().ui_font_size.clamp(12, 24) as f32,
                t!("assets.batch.title").to_string(),
            )
        });
        window.set_window_title(&title);
        let content = self.app.update(cx, |app, cx| {
            app.asset_batch_edit_window_view(
                (
                    f32::from(window.viewport_size().width),
                    f32::from(window.viewport_size().height),
                ),
                cx,
            )
        });
        let close_app = self.app.clone();
        let generation = self.generation;
        let on_close: ChildWindowCloseHandler =
            Rc::new(move |window: &mut Window, cx: &mut App| {
                close_app.update(cx, |app, cx| {
                    if app
                        .start_workspace
                        .asset_batch()
                        .matches_generation(generation)
                    {
                        app.dismiss_asset_batch_edit(cx);
                    }
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
    pub(in crate::features) fn activate_asset_batch_edit_window(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(handle) = self.start_workspace.asset_batch_window_handle() else {
            return false;
        };
        activate_child_window(
            &cx.entity(),
            handle,
            |app: &mut NyaTermApp| Some(app.start_workspace.asset_batch_window_slot()),
            cx,
        );
        true
    }

    pub(in crate::features) fn open_asset_batch_edit_window(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.activate_asset_batch_edit_window(cx) {
            return true;
        }
        if !self.start_workspace.begin_asset_batch_window_open() {
            return true;
        }
        cx.notify();
        let app = cx.entity();
        cx.defer(move |cx| {
            if app
                .read(cx)
                .start_workspace
                .asset_batch_window_open_pending()
            {
                open_asset_batch_edit_window_now_from_app(app, cx);
            }
        });
        true
    }
}

fn open_asset_batch_edit_window_now_from_app(app: Entity<NyaTermApp>, cx: &mut App) {
    if app
        .read(cx)
        .start_workspace
        .asset_batch_window_handle()
        .is_some()
    {
        app.update(cx, |app, cx| {
            app.start_workspace.cancel_asset_batch_window_open();
            app.activate_asset_batch_edit_window(cx);
            cx.notify();
        });
        return;
    }
    if !app.read(cx).start_workspace.asset_batch_is_open() {
        app.update(cx, |app, cx| {
            app.start_workspace.cancel_asset_batch_window_open();
            cx.notify();
        });
        return;
    }

    let title = t!("assets.batch.title").to_string();
    let spec = ChildWindowSpec::modal_editor(title, 900., 680.).min_size(420., 480.);
    let chrome = spec.chrome();
    let parent = app.read(cx).shell.main_window();
    let options = child_window_options(&spec, parent, cx);
    let generation = app.read(cx).start_workspace.asset_batch().generation();
    let close_app = app.clone();
    let view_app = app.clone();
    let result: anyhow::Result<NyaWindowHandle> = cx.open_window(options, move |window, cx| {
        window.on_window_should_close(cx, move |_, cx| {
            close_app.update(cx, |app, cx| {
                if app
                    .start_workspace
                    .asset_batch()
                    .matches_generation(generation)
                {
                    app.dismiss_asset_batch_edit(cx);
                }
            });
            true
        });
        let view = cx.new(|cx| AssetBatchEditWindow::new(view_app, chrome, cx));
        cx.new(|cx| nya_root(view, window, cx))
    });

    app.update(cx, |app, cx| match result {
        Ok(handle) => {
            app.start_workspace.finish_asset_batch_window_open(handle);
            cx.notify();
        }
        Err(error) => {
            app.start_workspace.clear_asset_batch_window();
            app.shell.set_status(error.to_string());
            cx.notify();
        }
    });
}
